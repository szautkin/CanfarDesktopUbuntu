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

/// One line per storage mutation: what was asked, where, and what came back.
///
/// The reference logs the same (`[Storage] MKDIR(node) PUT {url} -> {status}`),
/// and the reason is the bug this was added for: the app said "Created folder"
/// while the folder was invisible, and there was no way to tell from outside
/// whether the request had been made, where it went, or what the service said.
/// A toast reports the *app's* belief; this reports the exchange.
/// Whether an error means the node already exists.
///
/// VOSpace signals it as `409` with a `DuplicateNode` body; both are checked,
/// since the status alone is what the spec promises and the body is what makes
/// it unambiguous.
fn is_already_there(error: &ApiError) -> bool {
    match error {
        ApiError::Server { status, body } => *status == 409 || body.contains("DuplicateNode"),
        _ => false,
    }
}

fn log_storage(action: &str, url: &str, outcome: &Result<(), ApiError>) {
    match outcome {
        Ok(()) => eprintln!("[storage] {action} {url} -> ok"),
        Err(e) => eprintln!("[storage] {action} {url} -> FAILED: {e}"),
    }
}

/// The `setNode` body that creates a container (folder) at `node_uri`.
///
/// Separate from the request so the thing that was wrong can be tested: the
/// `uri` attribute must name the same node the PUT URL addresses, and nothing
/// about sending an HTTP request is needed to check that.
fn container_node_xml(node_uri: &str) -> String {
    format!(
        r#"<vos:node xmlns:vos="http://www.ivoa.net/xml/VOSpace/v2.0" uri="{node_uri}" xsi:type="vos:ContainerNode" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><vos:properties/><vos:nodes/></vos:node>"#
    )
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
        self.list_nodes_limited(token, username, path, None).await
    }

    /// List a container, asking the server for at most `limit` children.
    ///
    /// The cost of a listing is on VOSpace's side and scales with the number of
    /// sub-CONTAINERS, because it sizes each one. A caller that will only show
    /// the first N should say so rather than making the server compute — and
    /// the client wait for — a hundred it will discard.
    pub async fn list_nodes_limited(
        &self,
        token: &str,
        username: &str,
        path: &str,
        limit: Option<usize>,
    ) -> Result<Vec<VoSpaceNode>, ApiError> {
        let url = self
            .endpoints
            .vospace_nodes_url_limited(username, path, limit);
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

    /// Make sure a folder exists, without treating "it already does" as a
    /// failure.
    ///
    /// VOSpace answers a repeat `setNode` with `409 DuplicateNode`, and
    /// [`Self::create_folder`] reports that faithfully — which is right when a
    /// user asked to create a folder and one is already there, and wrong for
    /// the callers whose intent is "make sure the path is there". Those were
    /// swallowing the error and leaving a line reading
    /// `MKDIR … -> FAILED: DuplicateNode` in the log on every run: an alarming
    /// message about the expected case.
    ///
    /// Two methods rather than one lenient one, because the strict answer is
    /// load-bearing: the storage browser's New Folder must still tell the user
    /// their name is taken.
    pub async fn ensure_folder(
        &self,
        token: &str,
        username: &str,
        path: &str,
    ) -> Result<(), ApiError> {
        match self.create_folder(token, username, path).await {
            Err(e) if is_already_there(&e) => Ok(()),
            other => other,
        }
    }

    /// Create a folder at the given path under the user's home.
    pub async fn create_folder(
        &self,
        token: &str,
        username: &str,
        path: &str,
    ) -> Result<(), ApiError> {
        let url = self.endpoints.vospace_nodes_url(username, path);
        // The `uri` attribute must name the SAME node the URL addresses. It was
        // built here as `vos://cadc.nrc.ca~arc/{path}` while the URL rooted the
        // path under `home/{username}/` — so every folder creation asked the
        // service to make a node at a location that does not exist, and the
        // service answered "invalid URI". `vospace_node_uri` is the one place
        // that knows how a node is addressed, and `set_node_acl` already used it.
        let body = container_node_xml(&self.endpoints.vospace_node_uri(username, path));
        let resp = self
            .client
            .put(&url)
            .bearer_auth(token)
            .header("Content-Type", "text/xml")
            .body(body)
            .send()
            .await?;

        let outcome = check_response(resp).await.map(|_| ());
        // "Already there" is not a failure to report, whichever caller asked:
        // for `ensure_folder` it is the success case, and for `create_folder`
        // the CALLER turns it into a message for the user. Logging it as FAILED
        // put an alarming line in the log for the expected case.
        if outcome.as_ref().err().is_some_and(is_already_there) {
            eprintln!("[storage] MKDIR {url} -> already exists");
        } else {
            log_storage("MKDIR", &url, &outcome);
        }
        outcome
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
        let outcome = check_response(resp).await.map(|_| ());
        log_storage("SETNODE(acl)", &url, &outcome);
        outcome
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

        let outcome = check_response(resp).await.map(|_| ());
        log_storage("DELETE", &url, &outcome);
        outcome
    }

    /// Stream a local file to the given path, reporting progress and stopping
    /// when `cancel` is tripped.
    ///
    /// The whole-file variant below reads the source into memory first, which a
    /// multi-gigabyte cube cannot afford and which leaves nothing to report
    /// progress from.
    #[allow(clippy::too_many_arguments)]
    pub async fn upload_file_streaming(
        &self,
        token: &str,
        username: &str,
        path: &str,
        src: &std::path::Path,
        content_type: &str,
        progress: Option<crate::services::transfer::ProgressSink>,
        cancel: &crate::services::transfer::Cancel,
    ) -> Result<u64, crate::services::transfer::TransferError> {
        let url = self.endpoints.vospace_files_url(username, path);
        let outcome = crate::services::transfer::upload_from_file(
            &url,
            token,
            src,
            content_type,
            progress,
            cancel,
        )
        .await;
        log_storage(
            "PUT(stream)",
            &url,
            &outcome
                .as_ref()
                .map(|_| ())
                .map_err(|e| ApiError::Network(e.to_string())),
        );
        outcome
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
        let outcome = check_response(resp).await.map(|_| ());
        log_storage("PUT", &url, &outcome);
        outcome
    }

    /// Download a file's contents into memory (e.g. to parse a VOSpace-stored
    /// workflow document without touching disk).
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
        let outcome = check_response(resp).await.map(|_| ());
        log_storage("RENAME(copy)", &dst_url, &outcome);
        outcome?;

        // 3. Delete the old path
        self.delete_node(token, username, old_path).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn nobody_ignores_the_strict_create() {
        // `let _ = create_folder(...)` says "I do not care whether this worked",
        // which is never true — the caller cares about everything EXCEPT
        // already-exists, and that is what `ensure_folder` expresses. Four
        // callers wrote it the ignoring way, and between them logged
        // "MKDIR … -> FAILED: DuplicateNode" on every ordinary run while also
        // swallowing quota and permission errors without a word.
        let mut offenders: Vec<String> = Vec::new();
        for (path, text) in crate::testing::rust_sources() {
            let code = crate::testing::without_comments(crate::testing::code(&text));
            if code.contains("let _ = ") && code.contains(".create_folder(") {
                for line in code.lines() {
                    if line.contains("let _ =") && line.contains("create_folder") {
                        offenders.push(format!("{}: {}", path.display(), line.trim()));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "ignoring the result of the strict create — use ensure_folder: {offenders:#?}"
        );
    }

    #[test]
    fn the_strict_create_is_still_used_where_a_taken_name_matters() {
        // `ensure_folder` must not become the only one. When a person types a
        // folder name in the storage browser, or an agent asks for one, "that
        // name is taken" is the answer they need.
        let browser = crate::testing::rust_sources()
            .into_iter()
            .any(|(path, text)| {
                path.ends_with("vospace_browser.rs") && text.contains(".create_folder(")
            });
        assert!(browser, "New Folder no longer reports a name collision");
    }

    #[test]
    fn a_duplicate_node_means_the_folder_is_already_there() {
        // VOSpace answers a repeat setNode with 409 DuplicateNode. For a caller
        // whose intent is "make sure this path exists", that IS success.
        assert!(is_already_there(&ApiError::Server {
            status: 409,
            body: "DuplicateNode: vos://cadc.nrc.ca~arc/home/u/.verbinal".into(),
        }));
        // The status alone is what the spec promises; the body is what makes it
        // unambiguous. Either is enough.
        assert!(is_already_there(&ApiError::Server {
            status: 409,
            body: String::new(),
        }));
        assert!(is_already_there(&ApiError::Server {
            status: 500,
            body: "DuplicateNode".into(),
        }));
    }

    #[test]
    fn a_real_failure_is_still_a_failure() {
        // `ensure_folder` swallows exactly one condition. Quota, permissions and
        // an expired session must all still reach the caller.
        for error in [
            ApiError::Server {
                status: 403,
                body: "PermissionDenied".into(),
            },
            ApiError::Server {
                status: 500,
                body: "internal".into(),
            },
            ApiError::Unauthorized,
            ApiError::Network("timed out".into()),
        ] {
            assert!(!is_already_there(&error), "{error} was treated as success");
        }
    }
    use super::*;

    /// The node URI in the body must be the node the URL addresses.
    ///
    /// It was `vos://cadc.nrc.ca~arc/{path}` while the URL was
    /// `…/nodes/home/{username}/{path}` — two spellings of one address, and the
    /// service rejected every folder with "invalid URI". The bug was invisible
    /// because the body was built inside the request; it is a function now.
    #[test]
    fn a_new_folder_is_addressed_the_way_its_url_addresses_it() {
        let e = crate::config::ApiEndpoints::new(crate::config::AppConfig::default());
        let uri = e.vospace_node_uri("alice", "data/raw");
        let url = e.vospace_nodes_url("alice", "data/raw");

        // The URI names the same node as the URL: same tail, rooted the same way.
        assert!(uri.ends_with("home/alice/data/raw"), "{uri}");
        assert!(url.ends_with("home/alice/data/raw"), "{url}");

        let xml = container_node_xml(&uri);
        assert!(xml.contains(&format!(r#"uri="{uri}""#)), "{xml}");
        assert!(xml.contains(r#"xsi:type="vos:ContainerNode""#), "{xml}");
    }

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
