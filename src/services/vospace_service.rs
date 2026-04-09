use crate::config::ApiEndpoints;
use crate::helpers::vospace_parser;
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

    /// Get the download URL for a file.
    pub fn download_url(&self, username: &str, path: &str) -> String {
        self.endpoints.vospace_files_url(username, path)
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
}
