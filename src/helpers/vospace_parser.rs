use crate::models::vospace_node::{NodeType, VoSpaceNode};

/// Parse a VOSpace XML response into a list of child nodes.
pub fn parse_nodes(xml: &str) -> Result<Vec<VoSpaceNode>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("XML parse error: {}", e))?;

    let mut nodes = Vec::new();

    // Find all child <node> elements (direct children of the top-level <nodes>)
    for node in doc.descendants() {
        if node.tag_name().name() != "node" {
            continue;
        }
        // Skip the root node itself — we want its children
        if node
            .parent()
            .map(|p| p.tag_name().name() == "node")
            .unwrap_or(false)
        {
            continue;
        }
        // Iterate actual child nodes
        for child in node.children() {
            if child.tag_name().name() == "nodes" {
                for n in child.children() {
                    if n.tag_name().name() == "node" {
                        if let Some(parsed) = parse_single_node(&n) {
                            nodes.push(parsed);
                        }
                    }
                }
            }
        }
    }

    // Sort: folders first, then alphabetically
    nodes.sort_by(|a, b| {
        let a_dir = a.is_container();
        let b_dir = b.is_container();
        b_dir
            .cmp(&a_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(nodes)
}

fn parse_single_node(node: &roxmltree::Node) -> Option<VoSpaceNode> {
    let uri = node.attribute("uri")?.to_string();

    // Determine node type from xsi:type attribute
    let type_attr = node
        .attribute(("http://www.w3.org/2001/XMLSchema-instance", "type"))
        .or_else(|| node.attribute("type"))
        .unwrap_or("");

    let node_type = if type_attr.contains("ContainerNode") {
        NodeType::Container
    } else if type_attr.contains("LinkNode") {
        NodeType::Link
    } else {
        NodeType::Data
    };

    // Extract name from URI (last segment)
    let name = uri.rsplit('/').next().unwrap_or(&uri).to_string();

    // Parse properties
    let mut size: u64 = 0;
    let mut date: Option<String> = None;
    let mut content_type: Option<String> = None;

    for child in node.descendants() {
        if child.tag_name().name() == "property" {
            if let Some(prop_uri) = child.attribute("uri") {
                let text = child.text().unwrap_or("");
                if prop_uri.contains("length") {
                    size = text.parse().unwrap_or(0);
                } else if prop_uri.contains("date") {
                    date = Some(text.to_string());
                } else if prop_uri.contains("type") && !prop_uri.contains("groupread") {
                    content_type = Some(text.to_string());
                }
            }
        }
    }

    Some(VoSpaceNode {
        name,
        uri,
        node_type,
        size,
        date,
        content_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_nodes() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <vos:node xmlns:vos="http://www.ivoa.net/xml/VOSpace/v2.0" uri="vos://test/home/user">
            <vos:properties/>
            <vos:nodes/>
        </vos:node>"#;
        let result = parse_nodes(xml).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_with_children() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <vos:node xmlns:vos="http://www.ivoa.net/xml/VOSpace/v2.0"
                  xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                  uri="vos://test/home/user" xsi:type="vos:ContainerNode">
            <vos:properties/>
            <vos:nodes>
                <vos:node uri="vos://test/home/user/folder1" xsi:type="vos:ContainerNode">
                    <vos:properties/>
                </vos:node>
                <vos:node uri="vos://test/home/user/data.fits" xsi:type="vos:DataNode">
                    <vos:properties>
                        <vos:property uri="ivo://ivoa.net/vospace/core#length">1024</vos:property>
                    </vos:properties>
                </vos:node>
            </vos:nodes>
        </vos:node>"#;
        let result = parse_nodes(xml).unwrap();
        assert_eq!(result.len(), 2);
        // Folders first
        assert_eq!(result[0].name, "folder1");
        assert!(result[0].is_container());
        assert_eq!(result[1].name, "data.fits");
        assert_eq!(result[1].size, 1024);
    }
}
