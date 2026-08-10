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
    let mut is_public = false;
    let mut group_read: Vec<String> = Vec::new();
    let mut group_write: Vec<String> = Vec::new();

    // Only inspect this node's *own* <properties> child, not descendants —
    // otherwise a container's grandchild properties would leak into it.
    if let Some(props_el) = node
        .children()
        .find(|c| c.tag_name().name() == "properties")
    {
        for child in props_el.children() {
            if child.tag_name().name() != "property" {
                continue;
            }
            let Some(prop_uri) = child.attribute("uri") else {
                continue;
            };
            let text = child.text().unwrap_or("");
            if prop_uri.contains("#length") {
                size = text.parse().unwrap_or(0);
            } else if prop_uri.contains("#date") {
                date = Some(text.to_string());
            } else if prop_uri.contains("#contenttype") {
                content_type = Some(text.to_string());
            } else if prop_uri.contains("#ispublic") || prop_uri.contains("#publicread") {
                is_public = text.trim().eq_ignore_ascii_case("true");
            } else if prop_uri.contains("#groupread") {
                group_read = split_groups(text);
            } else if prop_uri.contains("#groupwrite") {
                group_write = split_groups(text);
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
        is_public,
        group_read,
        group_write,
    })
}

/// Split a whitespace-delimited list of GMS group URIs.
fn split_groups(text: &str) -> Vec<String> {
    text.split_whitespace().map(|s| s.to_string()).collect()
}

/// Parse a single-node document (e.g. the response to a per-node GET) into a
/// [`VoSpaceNode`] including its type and ACL, so a Share dialog can be prefilled.
pub fn parse_node(xml: &str) -> Result<VoSpaceNode, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("XML parse error: {}", e))?;
    for node in doc.descendants() {
        if node.tag_name().name() == "node" {
            return parse_single_node(&node).ok_or_else(|| "node has no uri".to_string());
        }
    }
    Err("no <node> element in document".to_string())
}

/// Escape a string for inclusion in XML text/attribute content.
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Build a VOSpace `setNode` (UpdateNode) body that sets access-control properties.
///
/// Per-dimension semantics mirror `Helpers/VoSpaceParser.BuildSetAclNodeXml`:
/// `None` = leave unchanged (property omitted), `Some(empty)` = revoke all groups
/// (empty property element), `Some(list)` = replace. The `is_public` bool emits
/// `#ispublic`.
///
/// 1.3.1 fix: a **ContainerNode** setNode document MUST carry the trailing
/// `<accepts/><provides/><capabilities/><nodes/>` child sequence or CADC's cavern
/// validator returns HTTP 400; a DataNode must omit it.
pub fn build_set_acl_node_xml(
    node_uri: &str,
    node_type: &NodeType,
    group_read: Option<&[String]>,
    group_write: Option<&[String]>,
    is_public: Option<bool>,
) -> String {
    let xsi_type = match node_type {
        NodeType::Container => "vos:ContainerNode",
        NodeType::Link => "vos:LinkNode",
        NodeType::Data => "vos:DataNode",
    };

    let mut props = String::new();
    if let Some(public) = is_public {
        props.push_str(&format!(
            "\n    <vos:property uri=\"ivo://ivoa.net/vospace/core#ispublic\">{}</vos:property>",
            public
        ));
    }
    if let Some(gr) = group_read {
        props.push_str(&format!(
            "\n    <vos:property uri=\"ivo://ivoa.net/vospace/core#groupread\">{}</vos:property>",
            xml_escape(&gr.join(" "))
        ));
    }
    if let Some(gw) = group_write {
        props.push_str(&format!(
            "\n    <vos:property uri=\"ivo://ivoa.net/vospace/core#groupwrite\">{}</vos:property>",
            xml_escape(&gw.join(" "))
        ));
    }

    let tail = if *node_type == NodeType::Container {
        "\n  <vos:accepts/>\n  <vos:provides/>\n  <vos:capabilities/>\n  <vos:nodes/>"
    } else {
        ""
    };

    format!(
        "<vos:node xmlns:vos=\"http://www.ivoa.net/xml/VOSpace/v2.0\" \
xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" uri=\"{}\" xsi:type=\"{}\">\n  \
<vos:properties>{}\n  </vos:properties>{}\n</vos:node>",
        xml_escape(node_uri),
        xsi_type,
        props,
        tail
    )
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

    #[test]
    fn parse_node_reads_acl() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <vos:node xmlns:vos="http://www.ivoa.net/xml/VOSpace/v2.0"
                  xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                  uri="vos://cadc.nrc.ca~arc/home/alice/shared" xsi:type="vos:ContainerNode">
            <vos:properties>
                <vos:property uri="ivo://ivoa.net/vospace/core#ispublic">true</vos:property>
                <vos:property uri="ivo://ivoa.net/vospace/core#groupread">ivo://cadc.nrc.ca/gms?A ivo://cadc.nrc.ca/gms?B</vos:property>
                <vos:property uri="ivo://ivoa.net/vospace/core#groupwrite">ivo://cadc.nrc.ca/gms?W</vos:property>
            </vos:properties>
            <vos:nodes/>
        </vos:node>"#;
        let node = parse_node(xml).unwrap();
        assert_eq!(node.name, "shared");
        assert!(node.is_container());
        assert!(node.is_public);
        assert_eq!(node.group_read.len(), 2);
        assert_eq!(node.group_read[0], "ivo://cadc.nrc.ca/gms?A");
        assert_eq!(
            node.group_write,
            vec!["ivo://cadc.nrc.ca/gms?W".to_string()]
        );
    }

    #[test]
    fn container_acl_body_has_tail() {
        let body = build_set_acl_node_xml(
            "vos://cadc.nrc.ca~arc/home/alice/shared",
            &NodeType::Container,
            Some(&["ivo://cadc.nrc.ca/gms?A".to_string()]),
            None,
            Some(true),
        );
        assert!(body.contains("xsi:type=\"vos:ContainerNode\""));
        assert!(body.contains("#ispublic\">true</vos:property>"));
        assert!(body.contains("#groupread\">ivo://cadc.nrc.ca/gms?A</vos:property>"));
        // groupwrite omitted (None = leave unchanged)
        assert!(!body.contains("#groupwrite"));
        // container tail required by cavern (the 1.3.1 fix)
        assert!(body.contains("<vos:accepts/>"));
        assert!(body.contains("<vos:provides/>"));
        assert!(body.contains("<vos:capabilities/>"));
        assert!(body.contains("<vos:nodes/>"));
    }

    #[test]
    fn data_acl_body_omits_tail() {
        let body = build_set_acl_node_xml(
            "vos://cadc.nrc.ca~arc/home/alice/data.fits",
            &NodeType::Data,
            Some(&[]), // revoke all read groups
            Some(&[]),
            Some(false),
        );
        assert!(body.contains("xsi:type=\"vos:DataNode\""));
        assert!(body.contains("#ispublic\">false</vos:property>"));
        // empty list => empty property element (revoke)
        assert!(body.contains("#groupread\"></vos:property>"));
        assert!(!body.contains("<vos:accepts/>"));
        assert!(!body.contains("<vos:nodes/>"));
    }

    #[test]
    fn acl_body_escapes_group_uris() {
        let body = build_set_acl_node_xml(
            "vos://cadc.nrc.ca~arc/home/a/x",
            &NodeType::Data,
            Some(&["ivo://x/gms?A&B".to_string()]),
            None,
            None,
        );
        assert!(body.contains("ivo://x/gms?A&amp;B"));
    }
}
