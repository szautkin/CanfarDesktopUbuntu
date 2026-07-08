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

    /// Get the download URL for a file.
    pub fn download_url(&self, username: &str, path: &str) -> String {
        self.endpoints.vospace_files_url(username, path)
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
