use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodeType {
    Container,
    Data,
    Link,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoSpaceNode {
    pub name: String,
    pub uri: String,
    pub node_type: NodeType,
    pub size: u64,
    pub date: Option<String>,
    pub content_type: Option<String>,
}

impl VoSpaceNode {
    pub fn is_container(&self) -> bool {
        self.node_type == NodeType::Container
    }

    pub fn size_display(&self) -> String {
        let bytes = self.size as f64;
        if bytes < 1024.0 {
            format!("{} B", self.size)
        } else if bytes < 1024.0 * 1024.0 {
            format!("{:.1} KB", bytes / 1024.0)
        } else if bytes < 1024.0 * 1024.0 * 1024.0 {
            format!("{:.1} MB", bytes / (1024.0 * 1024.0))
        } else {
            format!("{:.2} GB", bytes / (1024.0 * 1024.0 * 1024.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_display_bytes() {
        let node = VoSpaceNode {
            name: "test".into(),
            uri: "vos://test".into(),
            node_type: NodeType::Data,
            size: 512,
            date: None,
            content_type: None,
        };
        assert_eq!(node.size_display(), "512 B");
    }

    #[test]
    fn size_display_gb() {
        let node = VoSpaceNode {
            name: "test".into(),
            uri: "vos://test".into(),
            node_type: NodeType::Data,
            size: 2_147_483_648,
            date: None,
            content_type: None,
        };
        assert_eq!(node.size_display(), "2.00 GB");
    }

    #[test]
    fn is_container() {
        let folder = VoSpaceNode {
            name: "dir".into(),
            uri: "vos://dir".into(),
            node_type: NodeType::Container,
            size: 0,
            date: None,
            content_type: None,
        };
        assert!(folder.is_container());
    }
}
