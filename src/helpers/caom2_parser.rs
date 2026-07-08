//! Tolerant, namespace-agnostic CAOM-2 XML reader built on `roxmltree`.
//!
//! Port of `Services/CAOM2Parser.cs`. Element matching uses the local name
//! (`tag_name().name()`) so the document namespace prefix (`caom2:`, `vodml:`, …)
//! and schema-version drift (v2.4 / v2.5) don't matter. Unknown elements are
//! ignored rather than errored, so additive schema changes won't make an
//! observation unviewable.

use crate::models::caom2::{
    CAOM2Observation, Caom2Artifact, Caom2Environment, Caom2Instrument, Caom2Plane, Caom2Proposal,
    Caom2Target, Caom2Telescope,
};

type Node<'a> = roxmltree::Node<'a, 'a>;

/// Parse a CAOM-2 metadata document into a [`CAOM2Observation`].
///
/// Returns `Err` for malformed XML or a document missing the required
/// `collection` / `observationID` fields.
pub fn parse(xml: &str) -> Result<CAOM2Observation, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("Malformed CAOM2 XML: {e}"))?;
    parse_observation(doc.root_element())
}

fn parse_observation(el: Node) -> Result<CAOM2Observation, String> {
    let collection = text_child(el, "collection")
        .ok_or_else(|| "CAOM2 document missing required field: collection".to_string())?;
    let observation_id = text_child(el, "observationID")
        .ok_or_else(|| "CAOM2 document missing required field: observationID".to_string())?;

    Ok(CAOM2Observation {
        collection,
        observation_id,
        observation_type: text_child(el, "type"),
        intent: text_child(el, "intent"),
        sequence_number: text_child(el, "sequenceNumber"),
        algorithm: child(el, "algorithm").and_then(|a| text_child(a, "name")),
        proposal: child(el, "proposal").map(parse_proposal),
        target: child(el, "target").map(parse_target),
        telescope: child(el, "telescope").map(parse_telescope),
        instrument: child(el, "instrument").map(parse_instrument),
        environment: child(el, "environment").map(parse_environment),
        planes: child(el, "planes")
            .map(|p| children(p, "plane").into_iter().map(parse_plane).collect())
            .unwrap_or_default(),
    })
}

fn parse_proposal(el: Node) -> Caom2Proposal {
    Caom2Proposal {
        id: text_child(el, "id"),
        pi: text_child(el, "pi"),
        project: text_child(el, "project"),
        title: text_child(el, "title"),
        keywords: keyword_list(child(el, "keywords")),
    }
}

fn parse_target(el: Node) -> Caom2Target {
    Caom2Target {
        name: text_child(el, "name"),
        kind: text_child(el, "type"),
        standard: bool_child(el, "standard"),
        redshift: double_child(el, "redshift"),
        moving: bool_child(el, "moving"),
        keywords: keyword_list(child(el, "keywords")),
    }
}

fn parse_telescope(el: Node) -> Caom2Telescope {
    let x = double_child(el, "geoLocationX");
    let y = double_child(el, "geoLocationY");
    let z = double_child(el, "geoLocationZ");
    let geo = match (x, y, z) {
        (Some(a), Some(b), Some(c)) => Some((a, b, c)),
        _ => None,
    };
    Caom2Telescope {
        name: text_child(el, "name"),
        geo_location: geo,
        keywords: keyword_list(child(el, "keywords")),
    }
}

fn parse_instrument(el: Node) -> Caom2Instrument {
    Caom2Instrument {
        name: text_child(el, "name"),
        keywords: keyword_list(child(el, "keywords")),
    }
}

fn parse_environment(el: Node) -> Caom2Environment {
    Caom2Environment {
        seeing: double_child(el, "seeing"),
        humidity: double_child(el, "humidity"),
        elevation: double_child(el, "elevation"),
        tau: double_child(el, "tau"),
    }
}

fn parse_plane(el: Node) -> Caom2Plane {
    let (energy_lower, energy_upper) = match child(el, "energy") {
        Some(en) => parse_energy_bounds(en),
        None => (None, None),
    };
    let (time_lower, time_upper) = match child(el, "time").and_then(|t| child(t, "bounds")) {
        Some(b) => (double_child(b, "lower"), double_child(b, "upper")),
        None => (None, None),
    };

    Caom2Plane {
        product_id: text_child(el, "productID").unwrap_or_default(),
        calibration_level: int_child(el, "calibrationLevel"),
        data_product_type: text_child(el, "dataProductType"),
        quality: child(el, "quality").and_then(|q| text_child(q, "flag")),
        position_bounds: child(el, "position")
            .map(parse_position_bounds)
            .unwrap_or_default(),
        energy_lower,
        energy_upper,
        time_lower,
        time_upper,
        artifacts: child(el, "artifacts")
            .map(|a| {
                children(a, "artifact")
                    .into_iter()
                    .map(parse_artifact)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn parse_artifact(el: Node) -> Caom2Artifact {
    Caom2Artifact {
        uri: text_child(el, "uri").unwrap_or_default(),
        product_type: text_child(el, "productType"),
        content_type: text_child(el, "contentType"),
        content_length: u64_child(el, "contentLength"),
    }
}

/// Footprint polygon vertices. CAOM2 wraps the polygon under
/// `bounds/Polygon/points/vertex/{cval1,cval2}`; some documents flatten it to
/// `bounds/vertex`. Both are handled.
fn parse_position_bounds(pos: Node) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    if let Some(bounds) = child(pos, "bounds") {
        let poly_container = child(bounds, "Polygon").unwrap_or(bounds);
        if let Some(points) = child(poly_container, "points") {
            for v in children(points, "vertex") {
                if let Some(p) = parse_vertex(v) {
                    out.push(p);
                }
            }
        }
        for v in children(poly_container, "vertex") {
            if let Some(p) = parse_vertex(v) {
                out.push(p);
            }
        }
    }
    out
}

fn parse_vertex(el: Node) -> Option<(f64, f64)> {
    let ra = double_child(el, "cval1").or_else(|| double_child(el, "coord1"))?;
    let dec = double_child(el, "cval2").or_else(|| double_child(el, "coord2"))?;
    if ra.is_finite() && dec.is_finite() {
        Some((ra, dec))
    } else {
        None
    }
}

/// Energy bounds in metres, with a fallback to `axis/range/{start,end}/val`.
fn parse_energy_bounds(en: Node) -> (Option<f64>, Option<f64>) {
    let bounds = child(en, "bounds");
    let mut lower = bounds.and_then(|b| double_child(b, "lower"));
    let mut upper = bounds.and_then(|b| double_child(b, "upper"));
    if lower.is_none() || upper.is_none() {
        if let Some(range) = child(en, "axis").and_then(|a| child(a, "range")) {
            if lower.is_none() {
                lower = child(range, "start").and_then(|s| double_child(s, "val"));
            }
            if upper.is_none() {
                upper = child(range, "end").and_then(|e| double_child(e, "val"));
            }
        }
    }
    (lower, upper)
}

// ---- namespace-agnostic tree helpers (match by local name) -----------------

fn child<'a>(parent: Node<'a>, name: &str) -> Option<Node<'a>> {
    parent
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn children<'a>(parent: Node<'a>, name: &str) -> Vec<Node<'a>> {
    parent
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == name)
        .collect()
}

/// Concatenated text of a node's descendants (equivalent to `XElement.Value`),
/// trimmed; `None` when empty.
fn node_text(node: Node) -> String {
    let mut s = String::new();
    for d in node.descendants() {
        if d.is_text() {
            if let Some(t) = d.text() {
                s.push_str(t);
            }
        }
    }
    s
}

fn text_child(parent: Node, name: &str) -> Option<String> {
    let el = child(parent, name)?;
    let value = node_text(el);
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn double_child(parent: Node, name: &str) -> Option<f64> {
    text_child(parent, name)?.parse::<f64>().ok()
}

fn int_child(parent: Node, name: &str) -> Option<i32> {
    text_child(parent, name)?.parse::<i32>().ok()
}

fn u64_child(parent: Node, name: &str) -> Option<u64> {
    text_child(parent, name)?.parse::<u64>().ok()
}

fn bool_child(parent: Node, name: &str) -> Option<bool> {
    match text_child(parent, name)?.to_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// Keyword lists appear as `<keywords><keyword>…</keyword></keywords>` in some
/// schema versions and as a single space/`;`-separated string in others.
fn keyword_list(container: Option<Node>) -> Vec<String> {
    let Some(c) = container else {
        return Vec::new();
    };
    let elements: Vec<String> = children(c, "keyword")
        .into_iter()
        .map(|e| node_text(e).trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if !elements.is_empty() {
        return elements;
    }
    let raw = node_text(c);
    raw.split([' ', '\t', '\n', '\r', ';'])
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<caom2:Observation xmlns:caom2="http://www.opencadc.org/caom2/xml/v2.4">
  <caom2:collection>CFHT</caom2:collection>
  <caom2:observationID>1234567</caom2:observationID>
  <caom2:type>OBJECT</caom2:type>
  <caom2:intent>science</caom2:intent>
  <caom2:algorithm>
    <caom2:name>exposure</caom2:name>
  </caom2:algorithm>
  <caom2:proposal>
    <caom2:id>08AC01</caom2:id>
    <caom2:pi>Jane Doe</caom2:pi>
    <caom2:title>A Survey</caom2:title>
    <caom2:keywords>galaxy survey deep</caom2:keywords>
  </caom2:proposal>
  <caom2:target>
    <caom2:name>M31</caom2:name>
    <caom2:type>field</caom2:type>
    <caom2:redshift>0.001</caom2:redshift>
    <caom2:moving>false</caom2:moving>
  </caom2:target>
  <caom2:telescope>
    <caom2:name>CFHT 3.6m</caom2:name>
    <caom2:geoLocationX>-5464279.0</caom2:geoLocationX>
    <caom2:geoLocationY>-2493018.0</caom2:geoLocationY>
    <caom2:geoLocationZ>2150636.0</caom2:geoLocationZ>
  </caom2:telescope>
  <caom2:instrument>
    <caom2:name>MegaPrime</caom2:name>
  </caom2:instrument>
  <caom2:environment>
    <caom2:seeing>0.7</caom2:seeing>
  </caom2:environment>
  <caom2:planes>
    <caom2:plane>
      <caom2:productID>1234567p</caom2:productID>
      <caom2:calibrationLevel>2</caom2:calibrationLevel>
      <caom2:dataProductType>image</caom2:dataProductType>
      <caom2:quality>
        <caom2:flag>good</caom2:flag>
      </caom2:quality>
      <caom2:position>
        <caom2:bounds>
          <caom2:Polygon>
            <caom2:points>
              <caom2:vertex>
                <caom2:cval1>10.0</caom2:cval1>
                <caom2:cval2>41.0</caom2:cval2>
              </caom2:vertex>
              <caom2:vertex>
                <caom2:cval1>10.5</caom2:cval1>
                <caom2:cval2>41.5</caom2:cval2>
              </caom2:vertex>
            </caom2:points>
          </caom2:Polygon>
        </caom2:bounds>
      </caom2:position>
      <caom2:energy>
        <caom2:bounds>
          <caom2:lower>3.5e-7</caom2:lower>
          <caom2:upper>6.0e-7</caom2:upper>
        </caom2:bounds>
      </caom2:energy>
      <caom2:time>
        <caom2:bounds>
          <caom2:lower>56000.0</caom2:lower>
          <caom2:upper>56000.01</caom2:upper>
        </caom2:bounds>
      </caom2:time>
      <caom2:artifacts>
        <caom2:artifact>
          <caom2:uri>cadc:CFHT/1234567p.fits.fz</caom2:uri>
          <caom2:productType>science</caom2:productType>
          <caom2:contentType>application/fits</caom2:contentType>
          <caom2:contentLength>123456789</caom2:contentLength>
        </caom2:artifact>
      </caom2:artifacts>
    </caom2:plane>
  </caom2:planes>
</caom2:Observation>"#;

    #[test]
    fn parses_namespaced_document() {
        let obs = parse(SAMPLE).expect("parse ok");
        assert_eq!(obs.collection, "CFHT");
        assert_eq!(obs.observation_id, "1234567");
        assert_eq!(obs.observation_type.as_deref(), Some("OBJECT"));
        assert_eq!(obs.intent.as_deref(), Some("science"));
        assert_eq!(obs.algorithm.as_deref(), Some("exposure"));

        let proposal = obs.proposal.expect("proposal");
        assert_eq!(proposal.pi.as_deref(), Some("Jane Doe"));
        assert_eq!(proposal.keywords, vec!["galaxy", "survey", "deep"]);

        let target = obs.target.expect("target");
        assert_eq!(target.name.as_deref(), Some("M31"));
        assert_eq!(target.kind.as_deref(), Some("field"));
        assert_eq!(target.redshift, Some(0.001));
        assert_eq!(target.moving, Some(false));

        let telescope = obs.telescope.expect("telescope");
        assert_eq!(
            telescope.geo_location,
            Some((-5464279.0, -2493018.0, 2150636.0))
        );

        assert_eq!(obs.instrument.and_then(|i| i.name).as_deref(), Some("MegaPrime"));
        assert_eq!(obs.environment.and_then(|e| e.seeing), Some(0.7));

        assert_eq!(obs.planes.len(), 1);
        let plane = &obs.planes[0];
        assert_eq!(plane.product_id, "1234567p");
        assert_eq!(plane.calibration_level, Some(2));
        assert_eq!(plane.data_product_type.as_deref(), Some("image"));
        assert_eq!(plane.quality.as_deref(), Some("good"));
        assert_eq!(plane.position_bounds, vec![(10.0, 41.0), (10.5, 41.5)]);
        assert_eq!(plane.energy_lower, Some(3.5e-7));
        assert_eq!(plane.energy_upper, Some(6.0e-7));
        assert_eq!(plane.time_lower, Some(56000.0));
        assert_eq!(plane.time_upper, Some(56000.01));

        assert_eq!(plane.artifacts.len(), 1);
        let artifact = &plane.artifacts[0];
        assert_eq!(artifact.uri, "cadc:CFHT/1234567p.fits.fz");
        assert_eq!(artifact.product_type.as_deref(), Some("science"));
        assert_eq!(artifact.content_type.as_deref(), Some("application/fits"));
        assert_eq!(artifact.content_length, Some(123_456_789));
    }

    #[test]
    fn missing_required_field_errors() {
        let xml = r#"<Observation><collection>CFHT</collection></Observation>"#;
        assert!(parse(xml).is_err());
    }

    #[test]
    fn malformed_xml_errors() {
        assert!(parse("<Observation><oops").is_err());
    }
}
