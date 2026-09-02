use crate::config::ApiEndpoints;
use crate::models::search_result::{DataLinkFile, DataLinkResult};
use crate::services::api_error::ApiError;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

pub struct DataLinkService {
    client: Client,
    endpoints: Arc<ApiEndpoints>,
    cache: Mutex<HashMap<String, DataLinkResult>>,
    image_semaphore: Semaphore,
}

impl DataLinkService {
    pub fn new(client: Client, endpoints: Arc<ApiEndpoints>) -> Self {
        DataLinkService {
            client,
            endpoints,
            cache: Mutex::new(HashMap::new()),
            image_semaphore: Semaphore::new(3), // max 3 concurrent image downloads
        }
    }

    /// Resolve DataLink for a given publisherID. Returns cached result if available.
    pub async fn resolve(
        &self,
        publisher_id: &str,
        token: Option<&str>,
    ) -> Result<DataLinkResult, ApiError> {
        // Check cache
        {
            let cache = self.cache.lock().await;
            if let Some(cached) = cache.get(publisher_id) {
                return Ok(cached.clone());
            }
        }

        let url = format!(
            "{}?id={}&request=downloads-only",
            self.endpoints.datalink_base_url(),
            urlencoding::encode(publisher_id)
        );

        let mut req = self
            .client
            .get(&url)
            .header("Accept", "application/x-votable+xml")
            .timeout(std::time::Duration::from_secs(30));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await?;

        if !resp.status().is_success() {
            return Err(ApiError::Server {
                status: resp.status().as_u16(),
                body: format!("DataLink failed for {}", publisher_id),
            });
        }

        let xml = resp
            .text()
            .await
            .map_err(|e| ApiError::Parse(e.to_string()))?;

        let (mut result, faults) = parse_votable(&xml, publisher_id);

        // Every row was a fault. The service answered — 200, a well-formed
        // VOTable — and said it could not use this id. Reporting that as a
        // resolve with no files sent the caller to the package endpoint, which
        // answers 200 with an EMPTY BODY for an id it cannot resolve, and a
        // zero-byte file was written and recorded as a successful download.
        if result.files.is_empty() && !faults.is_empty() {
            return Err(ApiError::Parse(format!(
                "DataLink rejected {publisher_id}: {}",
                faults.join("; ")
            )));
        }
        result.download_url = Some(format!(
            "{}?ID={}",
            self.endpoints.pkg_url(),
            urlencoding::encode(publisher_id)
        ));

        // Cache successful result
        {
            let mut cache = self.cache.lock().await;
            cache.insert(publisher_id.to_string(), result.clone());
        }

        Ok(result)
    }

    /// Download URL for direct package download (no DataLink resolution needed).
    pub fn download_url(&self, publisher_id: &str) -> String {
        format!(
            "{}?ID={}",
            self.endpoints.pkg_url(),
            urlencoding::encode(publisher_id)
        )
    }

    /// Download a thumbnail/preview image with concurrency limiting.
    /// Retries once with 300ms delay on failure (matching Windows).
    ///
    /// The one place this service buffers a whole body, and deliberately: a
    /// preview is a few hundred kilobytes and its bytes go straight into a
    /// texture. Science files are STREAMED to disk by
    /// [`download_to_file`](crate::services::transfer::download_to_file)
    /// — a sibling of this function that buffered them into a `Vec<u8>` was
    /// deleted unused, which is the only reason it never met a multi-gigabyte
    /// cube.
    pub async fn download_image(
        &self,
        url: &str,
        token: Option<&str>,
    ) -> Result<Vec<u8>, ApiError> {
        let _permit = self.image_semaphore.acquire().await.unwrap();

        let do_request = |client: &Client, url: &str, token: Option<&str>| {
            let mut req = client.get(url).timeout(std::time::Duration::from_secs(15));
            if let Some(t) = token {
                req = req.bearer_auth(t);
            }
            req.send()
        };

        let resp = match do_request(&self.client, url, token).await {
            Ok(r) => r,
            Err(_) => {
                // Retry once after 300ms
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                do_request(&self.client, url, token).await?
            }
        };

        if !resp.status().is_success() {
            return Err(ApiError::Server {
                status: resp.status().as_u16(),
                body: "Image download failed".to_string(),
            });
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        Ok(bytes.to_vec())
    }
}

/// Parse a VOTable XML response to extract DataLink files.
///
/// Returns the rows AND the faults it skipped. A DataLink response is a table:
/// some rows can carry data while others carry an `error_message`, so a fault
/// is not automatically fatal — but a response that is ONLY faults is, and the
/// caller could not tell the two apart when the faults were dropped silently.
fn parse_votable(xml: &str, publisher_id: &str) -> (DataLinkResult, Vec<String>) {
    let mut files = Vec::new();
    let mut faults: Vec<String> = Vec::new();

    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return (
            DataLinkResult {
                publisher_id: publisher_id.to_string(),
                files,
                download_url: None,
            },
            faults,
        );
    };

    // Find FIELD elements to determine column indices
    let mut col_url = None;
    let mut col_semantics = None;
    let mut col_content_type = None;
    let mut col_content_length = None;
    let mut col_description = None;
    let mut col_error = None;

    let mut field_idx = 0;
    for node in doc.descendants() {
        if node.tag_name().name() == "FIELD" {
            let name = node.attribute("name").unwrap_or("").to_lowercase();
            match name.as_str() {
                "access_url" => col_url = Some(field_idx),
                "semantics" => col_semantics = Some(field_idx),
                "content_type" => col_content_type = Some(field_idx),
                "content_length" => col_content_length = Some(field_idx),
                "description" => col_description = Some(field_idx),
                "error_message" => col_error = Some(field_idx),
                _ => {}
            }
            field_idx += 1;
        }
    }

    // Parse TR rows
    for tr in doc.descendants().filter(|n| n.tag_name().name() == "TR") {
        let tds: Vec<String> = tr
            .children()
            .filter(|n| n.tag_name().name() == "TD")
            .map(|n| n.text().unwrap_or("").to_string())
            .collect();

        // An error row carries no data — record why, then skip it. Dropping
        // these silently is how an "invalid ID" answer became an empty result
        // that read as a successful resolve with nothing in it.
        if let Some(ei) = col_error {
            if let Some(error_msg) = tds.get(ei) {
                if !error_msg.trim().is_empty() {
                    faults.push(error_msg.trim().to_string());
                    continue;
                }
            }
        }

        let url = col_url
            .and_then(|i| tds.get(i))
            .cloned()
            .unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        // Security: drop any non-HTTPS access_url (downgrade / SSRF defence).
        if !url.trim().to_ascii_lowercase().starts_with("https://") {
            continue;
        }

        let semantics = col_semantics
            .and_then(|i| tds.get(i))
            .cloned()
            .unwrap_or_default();
        let content_type = col_content_type
            .and_then(|i| tds.get(i))
            .cloned()
            .unwrap_or_default();
        let size_str = col_content_length
            .and_then(|i| tds.get(i))
            .cloned()
            .unwrap_or_default();
        let description = col_description
            .and_then(|i| tds.get(i))
            .cloned()
            .unwrap_or_default();

        let size = size_str.trim().parse::<u64>().ok();

        files.push(DataLinkFile {
            url,
            semantics,
            content_type,
            size,
            description,
        });
    }

    (
        DataLinkResult {
            publisher_id: publisher_id.to_string(),
            files,
            download_url: None,
        },
        faults,
    )
}

#[cfg(test)]
mod tests {
    /// A response that is only faults yields no rows and reports why.
    ///
    /// This is the JWST 0-byte bug in one place. CADC mirrors JWST, so a real
    /// publisher id carries a `mirror/` segment and the observation id as a
    /// path step; an id assembled without them gets
    /// `UsageFault: invalid ID`. The fault row used to be skipped in silence,
    /// leaving an empty file list that read as "resolved, nothing here" — so
    /// the caller fell through to the package endpoint, which answers HTTP 200
    /// with an EMPTY BODY for an id it cannot resolve, and a zero-byte file was
    /// written and recorded as a successful download.
    #[test]
    fn a_response_of_only_faults_reports_the_fault() {
        let xml = r#"<?xml version="1.0"?>
<VOTABLE><RESOURCE><TABLE>
  <FIELD name="ID" datatype="char"/>
  <FIELD name="access_url" datatype="char"/>
  <FIELD name="semantics" datatype="char"/>
  <FIELD name="error_message" datatype="char"/>
  <DATA><TABLEDATA>
    <TR><TD>ivo://cadc.nrc.ca/JWST?jw03435-PRODUCT</TD><TD></TD><TD>#this</TD><TD>UsageFault: invalid ID: ivo://cadc.nrc.ca/JWST?jw03435-PRODUCT</TD></TR>
  </TABLEDATA></DATA>
</TABLE></RESOURCE></VOTABLE>"#;

        let (result, faults) = parse_votable(xml, "ivo://cadc.nrc.ca/JWST?jw03435-PRODUCT");
        assert!(result.files.is_empty(), "a fault row is not a file");
        assert_eq!(faults.len(), 1, "the fault was dropped: {faults:?}");
        assert!(faults[0].contains("UsageFault"), "{faults:?}");
    }

    /// A fault BESIDE real rows is not fatal.
    ///
    /// DataLink is a table: one artifact can be unavailable while the others
    /// are fine. Refusing the whole response over one bad row would break every
    /// observation with a single restricted product.
    #[test]
    fn a_fault_beside_real_rows_does_not_discard_them() {
        let xml = r#"<?xml version="1.0"?>
<VOTABLE><RESOURCE><TABLE>
  <FIELD name="ID" datatype="char"/>
  <FIELD name="access_url" datatype="char"/>
  <FIELD name="semantics" datatype="char"/>
  <FIELD name="error_message" datatype="char"/>
  <DATA><TABLEDATA>
    <TR><TD>x</TD><TD></TD><TD>#this</TD><TD>NotFoundFault: gone</TD></TR>
    <TR><TD>x</TD><TD>https://example.org/a_i2d.fits</TD><TD>#this</TD><TD></TD></TR>
  </TABLEDATA></DATA>
</TABLE></RESOURCE></VOTABLE>"#;

        let (result, faults) = parse_votable(xml, "x");
        assert_eq!(result.files.len(), 1, "the good row was discarded");
        assert_eq!(faults.len(), 1, "the fault should still be recorded");
    }

    use super::*;

    #[test]
    fn parse_empty_votable() {
        let xml = r#"<?xml version="1.0"?><VOTABLE><RESOURCE><TABLE></TABLE></RESOURCE></VOTABLE>"#;
        let (result, _faults) = parse_votable(xml, "test:id");
        assert!(result.files.is_empty());
    }

    #[test]
    fn parse_votable_with_files() {
        let xml = r#"<?xml version="1.0"?>
        <VOTABLE>
        <RESOURCE type="results">
        <TABLE>
            <FIELD name="access_url" datatype="char"/>
            <FIELD name="semantics" datatype="char"/>
            <FIELD name="content_type" datatype="char"/>
            <FIELD name="content_length" datatype="long"/>
            <FIELD name="description" datatype="char"/>
            <DATA><TABLEDATA>
                <TR>
                    <TD>https://example.com/file.fits</TD>
                    <TD>#this</TD>
                    <TD>application/fits</TD>
                    <TD>1048576</TD>
                    <TD>Science data</TD>
                </TR>
                <TR>
                    <TD>https://example.com/thumb.jpg</TD>
                    <TD>#thumbnail</TD>
                    <TD>image/jpeg</TD>
                    <TD>5000</TD>
                    <TD>Thumbnail</TD>
                </TR>
            </TABLEDATA></DATA>
        </TABLE>
        </RESOURCE>
        </VOTABLE>"#;

        let (result, _faults) = parse_votable(xml, "ivo://test");
        assert_eq!(result.files.len(), 2);
        assert!(result.files[0].is_science_data());
        assert_eq!(result.files[0].size, Some(1048576));
        assert!(result.files[1].is_thumbnail());
    }

    #[test]
    fn parse_votable_skips_error_rows() {
        let xml = r#"<?xml version="1.0"?>
        <VOTABLE>
        <RESOURCE>
        <TABLE>
            <FIELD name="access_url" datatype="char"/>
            <FIELD name="semantics" datatype="char"/>
            <FIELD name="error_message" datatype="char"/>
            <DATA><TABLEDATA>
                <TR><TD>https://ok.com/f.fits</TD><TD>#this</TD><TD></TD></TR>
                <TR><TD>https://bad.com/f.fits</TD><TD>#this</TD><TD>Not found</TD></TR>
            </TABLEDATA></DATA>
        </TABLE>
        </RESOURCE>
        </VOTABLE>"#;

        let (result, _faults) = parse_votable(xml, "test");
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].url, "https://ok.com/f.fits");
    }
}
