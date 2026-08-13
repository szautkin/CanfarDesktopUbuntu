use crate::config::ApiEndpoints;
use crate::helpers::vospace_parser;
use crate::models::vospace_node::NodeType;
use crate::models::VoSpaceNode;
use crate::services::api_error::{check_response, ApiError};
use reqwest::Client;
use std::sync::Arc;

pub struct VoSpaceService {
    client: Client,
    endpoints: Arc<ApiEndpoints>,
}

/// Result of a bounded download: the bytes actually fetched, whether the file
/// continued past them, and the declared total when the server sent one.
pub struct LimitedDownload {
    pub bytes: Vec<u8>,
    /// True when the file had more data past `max_bytes`.
    pub truncated: bool,
    /// `Content-Length`, when the server declared it. `None` for chunked
    /// responses — the caller then knows a size only if the read finished.
    pub total_bytes: Option<u64>,
}

/// Accumulates streamed chunks up to a byte ceiling.
///
/// Split out from the transport so the truncation rule can be tested without a
/// network: the boundary cases (a file that ends exactly at the limit versus one
/// that continues past it) are precisely where an off-by-one hides, and they are
/// invisible in an integration test against a live VOSpace.
struct BoundedReader {
    buf: Vec<u8>,
    max_bytes: usize,
    saw_more: bool,
}

impl BoundedReader {
    fn new(max_bytes: usize) -> Self {
        BoundedReader {
            buf: Vec::with_capacity(max_bytes.min(64 * 1024)),
            max_bytes,
            saw_more: false,
        }
    }

    /// Fold one chunk in. Returns `true` once the caller should stop reading.
    ///
    /// A buffer that lands exactly ON the limit does NOT stop: the file may end
    /// there, and only the next read (a chunk, or end-of-stream) distinguishes
    /// "exactly max_bytes long" from "truncated". This mirrors the reference's
    /// probe-one-more-byte step.
    fn push(&mut self, chunk: &[u8]) -> bool {
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > self.max_bytes {
            self.buf.truncate(self.max_bytes);
            self.saw_more = true;
            return true;
        }
        false
    }

    /// The bytes read and whether anything followed them.
    fn finish(self) -> (Vec<u8>, bool) {
        (self.buf, self.saw_more)
    }
}

impl VoSpaceService {
    pub fn new(client: Client, endpoints: Arc<ApiEndpoints>) -> Self {
        VoSpaceService { client, endpoints }
    }

    /// List nodes (files and folders) at the given path under the user's home.
    pub async fn list_nodes(
        &self,
        token: &str,
        username: &str,
        path: &str,
    ) -> Result<Vec<VoSpaceNode>, ApiError> {
        let url = self.endpoints.vospace_nodes_url(username, path);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(token)
            .header("Accept", "text/xml")
            .send()
            .await?;

        let resp = check_response(resp).await?;
        let xml_text = resp
            .text()
            .await
            .map_err(|e| ApiError::Parse(e.to_string()))?;
        vospace_parser::parse_nodes(&xml_text).map_err(ApiError::Parse)
    }

    /// Create a folder at the given path under the user's home.
    pub async fn create_folder(
        &self,
        token: &str,
        username: &str,
        path: &str,
    ) -> Result<(), ApiError> {
        let url = self.endpoints.vospace_nodes_url(username, path);
        let body = format!(
            r#"<vos:node xmlns:vos="http://www.ivoa.net/xml/VOSpace/v2.0" uri="vos://cadc.nrc.ca~arc/{}" xsi:type="vos:ContainerNode" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><vos:properties/><vos:nodes/></vos:node>"#,
            path
        );
        let resp = self
            .client
            .put(&url)
            .bearer_auth(token)
            .header("Content-Type", "text/xml")
            .body(body)
            .send()
            .await?;

        check_response(resp).await?;
        Ok(())
    }

    /// Fetch a single node's metadata (type + ACL) so a Share dialog can be
    /// prefilled with the node's current access-control state.
    pub async fn get_node(
        &self,
        token: &str,
        username: &str,
        path: &str,
    ) -> Result<VoSpaceNode, ApiError> {
        let url = self.endpoints.vospace_nodes_url(username, path);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(token)
            .header("Accept", "text/xml")
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let xml = resp
            .text()
            .await
            .map_err(|e| ApiError::Parse(e.to_string()))?;
        vospace_parser::parse_node(&xml).map_err(ApiError::Parse)
    }

    /// Set the access-control properties (public flag + read/write groups) of a
    /// node via VOSpace `setNode` (HTTP POST). `None` for a dimension leaves it
    /// unchanged; `Some(empty)` revokes; `Some(list)` replaces.
    ///
    /// `node_type` must echo the node's existing type — it comes from the listed
    /// node or a prior [`get_node`].
    pub async fn set_node_acl(
        &self,
        token: &str,
        username: &str,
        path: &str,
        node_type: &NodeType,
        group_read: Option<Vec<String>>,
        group_write: Option<Vec<String>>,
        is_public: Option<bool>,
    ) -> Result<(), ApiError> {
        let url = self.endpoints.vospace_nodes_url(username, path);
        let node_uri = self.endpoints.vospace_node_uri(username, path);
        let body = vospace_parser::build_set_acl_node_xml(
            &node_uri,
            node_type,
            group_read.as_deref(),
            group_write.as_deref(),
            is_public,
        );
        let resp = self
            .client
            .post(&url)
            .bearer_auth(token)
            .header("Content-Type", "text/xml")
            .body(body)
            .send()
            .await?;
        check_response(resp).await?;
        Ok(())
    }

    /// Delete a node (file or folder) at the given path.
    pub async fn delete_node(
        &self,
        token: &str,
        username: &str,
        path: &str,
    ) -> Result<(), ApiError> {
        let url = self.endpoints.vospace_nodes_url(username, path);
        let resp = self.client.delete(&url).bearer_auth(token).send().await?;

        check_response(resp).await?;
        Ok(())
    }

    /// Upload a file to the given path under the user's home.
    pub async fn upload_file(
        &self,
        token: &str,
        username: &str,
        path: &str,
        data: Vec<u8>,
        content_type: &str,
    ) -> Result<(), ApiError> {
        let url = self.endpoints.vospace_files_url(username, path);
        let resp = self
            .client
            .put(&url)
            .bearer_auth(token)
            .header("Content-Type", content_type)
            .body(data)
            .send()
            .await?;
        check_response(resp).await?;
        Ok(())
    }

    /// Download a file's contents into memory (e.g. to parse a VOSpace-stored
    /// workflow document without touching disk).
    #[allow(dead_code)]
    pub async fn download_bytes(
        &self,
        token: &str,
        username: &str,
        path: &str,
    ) -> Result<Vec<u8>, ApiError> {
        let url = self.endpoints.vospace_files_url(username, path);
        let resp = self.client.get(&url).bearer_auth(token).send().await?;
        let resp = check_response(resp).await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        Ok(bytes.to_vec())
    }

    /// Download at most `max_bytes` of a file, stopping the transfer there.
    ///
    /// [`Self::download_bytes`] buffers the entire response, which is the wrong
    /// shape for a bounded read: `read_vospace_file` asking for 64 KB of a
    /// multi-gigabyte cube would pull the whole cube into memory before slicing
    /// off the front. This streams instead and drops the connection as soon as
    /// it has enough, so the cost is bounded by what the caller asked for.
    pub async fn download_bytes_limited(
        &self,
        token: &str,
        username: &str,
        path: &str,
        max_bytes: usize,
    ) -> Result<LimitedDownload, ApiError> {
        let url = self.endpoints.vospace_files_url(username, path);
        let resp = self.client.get(&url).bearer_auth(token).send().await?;
        let mut resp = check_response(resp).await?;

        // Declared up front by most servers; the only way a bounded read can
        // report how much it did NOT fetch.
        let total_bytes = resp.content_length();

        let mut reader = BoundedReader::new(max_bytes);
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?
        {
            if reader.push(&chunk) {
                break;
            }
        }

        let (bytes, truncated) = reader.finish();
        Ok(LimitedDownload {
            bytes,
            truncated,
            total_bytes,
        })
    }

    /// Download a file to a local path.
    pub async fn download_file(
        &self,
        token: &str,
        username: &str,
        remote_path: &str,
        local_path: &std::path::Path,
    ) -> Result<u64, ApiError> {
        let url = self.endpoints.vospace_files_url(username, remote_path);
        let resp = self.client.get(&url).bearer_auth(token).send().await?;

        let resp = check_response(resp).await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        let len = bytes.len() as u64;
        std::fs::write(local_path, &bytes)
            .map_err(|e| ApiError::Network(format!("Write error: {}", e)))?;
        Ok(len)
    }

    /// Rename a file node via copy+delete.
    ///
    /// This is an MVP implementation that works only for regular files.
    /// The proper VOSpace `transferNodes` move API is deferred.
    ///
    /// - `old_path`: current path of the file (relative to the user's home)
    /// - `new_name`: new basename (not a full path)
    pub async fn rename_file(
        &self,
        token: &str,
        username: &str,
        old_path: &str,
        new_name: &str,
    ) -> Result<(), ApiError> {
        // Build the new path by replacing the final segment of old_path
        let trimmed = old_path.trim_end_matches('/');
        let parent = trimmed.rfind('/').map(|i| &trimmed[..i]).unwrap_or("");
        let new_path = if parent.is_empty() {
            new_name.to_string()
        } else {
            format!("{}/{}", parent, new_name)
        };

        // 1. Download the source bytes
        let src_url = self.endpoints.vospace_files_url(username, old_path);
        let resp = self.client.get(&src_url).bearer_auth(token).send().await?;
        let resp = check_response(resp).await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        // 2. Upload to the new path
        let dst_url = self.endpoints.vospace_files_url(username, &new_path);
        let resp = self
            .client
            .put(&dst_url)
            .bearer_auth(token)
            .header("Content-Type", "application/octet-stream")
            .body(bytes.to_vec())
            .send()
            .await?;
        check_response(resp).await?;

        // 3. Delete the old path
        self.delete_node(token, username, old_path).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed chunks through a reader the way the transport loop does, stopping
    /// when it says to.
    fn read_all(chunks: &[&[u8]], max_bytes: usize) -> (Vec<u8>, bool) {
        let mut reader = BoundedReader::new(max_bytes);
        for chunk in chunks {
            if reader.push(chunk) {
                break;
            }
        }
        reader.finish()
    }

    #[test]
    fn a_short_file_is_returned_whole_and_untruncated() {
        let (bytes, truncated) = read_all(&[b"hello ", b"world"], 1024);
        assert_eq!(bytes, b"hello world");
        assert!(!truncated);
    }

    #[test]
    fn a_file_ending_exactly_on_the_limit_is_not_truncated() {
        // The boundary that matters: 11 bytes read with an 11-byte ceiling is a
        // COMPLETE file. Reporting it as truncated would send the caller back
        // for a second read that returns nothing.
        let (bytes, truncated) = read_all(&[b"hello world"], 11);
        assert_eq!(bytes.len(), 11);
        assert!(!truncated, "the file ended exactly at the limit");
    }

    #[test]
    fn one_byte_past_the_limit_is_truncated() {
        let (bytes, truncated) = read_all(&[b"hello world!"], 11);
        assert_eq!(bytes, b"hello world");
        assert!(truncated);
    }

    #[test]
    fn the_limit_holds_across_chunk_boundaries() {
        // The realistic case: the ceiling falls inside a chunk, not on its edge.
        let (bytes, truncated) = read_all(&[b"aaaa", b"bbbb", b"cccc"], 6);
        assert_eq!(bytes, b"aaaabb");
        assert!(truncated);
    }

    #[test]
    fn reading_stops_as_soon_as_the_limit_is_passed() {
        // The whole point of streaming: later chunks are never pulled. A reader
        // that kept consuming would defeat the bound on a huge file.
        let mut reader = BoundedReader::new(4);
        assert!(!reader.push(b"ab"), "still under the limit");
        assert!(reader.push(b"cdef"), "past the limit — stop now");
    }

    #[test]
    fn a_zero_limit_reads_nothing_but_still_reports_more() {
        let (bytes, truncated) = read_all(&[b"data"], 0);
        assert!(bytes.is_empty());
        assert!(truncated);
    }

    #[test]
    fn an_empty_file_is_empty_and_untruncated() {
        let (bytes, truncated) = read_all(&[], 1024);
        assert!(bytes.is_empty());
        assert!(!truncated);
    }
}
