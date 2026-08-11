//! Tolerant, namespace-agnostic CAOM-2 XML reader built on `roxmltree`.
//!
//! Port of `Services/CAOM2Parser.cs`. Element matching uses the local name
//! (`tag_name().name()`) so the document namespace prefix (`caom2:`, `vodml:`, …)
//! and schema-version drift (v2.4 / v2.5) don't matter. Unknown elements are
//! ignored rather than errored, so additive schema changes won't make an
//! observation unviewable.

use crate::models::caom2::{
    CAOM2Observation, Caom2Artifact, Caom2Environment, Caom2Instrument, Caom2Plane, Caom2Proposal,
    Caom2Provenance, Caom2Target, Caom2Telescope,
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
        meta_release: text_child(el, "metaRelease"),
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
        ambient_temp: double_child(el, "ambientTemp"),
        photometric: bool_child(el, "photometric"),
    }
}

fn parse_plane(el: Node) -> Caom2Plane {
    let energy = child(el, "energy");
    let (energy_lower, energy_upper) = match energy {
        Some(en) => parse_energy_bounds(en),
        None => (None, None),
    };
    let position = child(el, "position");
    let time = child(el, "time");
    let (time_lower, time_upper) = match time.and_then(|t| child(t, "bounds")) {
        Some(b) => (double_child(b, "lower"), double_child(b, "upper")),
        None => (None, None),
    };

    Caom2Plane {
        product_id: text_child(el, "productID").unwrap_or_default(),
        calibration_level: int_child(el, "calibrationLevel"),
        data_product_type: text_child(el, "dataProductType"),
        quality: child(el, "quality").and_then(|q| text_child(q, "flag")),
        creator_id: text_child(el, "creatorID"),
        meta_release: text_child(el, "metaRelease"),
        data_release: text_child(el, "dataRelease"),
        provenance: child(el, "provenance").map(parse_provenance),
        position_bounds: position.map(parse_position_bounds).unwrap_or_default(),
        position_dimension: position
            .and_then(|p| child(p, "dimension"))
            .and_then(parse_dimension),
        position_resolution: position.and_then(|p| double_child(p, "resolution")),
        position_sample_size: position.and_then(|p| double_child(p, "sampleSize")),
        energy_lower,
        energy_upper,
        energy_bandpass: energy.and_then(|en| text_child(en, "bandpassName")),
        energy_em_band: energy.and_then(|en| text_child(en, "emBand")),
        energy_resolving_power: energy.and_then(|en| double_child(en, "resolvingPower")),
        energy_rest_wav: energy.and_then(|en| double_child(en, "restwav")),
        time_lower,
        time_upper,
        time_exposure: time.and_then(|t| double_child(t, "exposure")),
        polarization_states: child(el, "polarization")
            .map(|pol| text_value_list(pol, "states", "state"))
            .unwrap_or_default(),
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

fn parse_provenance(el: Node) -> Caom2Provenance {
    Caom2Provenance {
        name: text_child(el, "name"),
        version: text_child(el, "version"),
        project: text_child(el, "project"),
        producer: text_child(el, "producer"),
        run_id: text_child(el, "runID"),
        reference: text_child(el, "reference"),
        last_executed: text_child(el, "lastExecuted"),
        inputs: text_value_list(el, "inputs", "planeURI"),
    }
}

/// Pixel dimensions `naxis1 × naxis2`, or `None` when either axis is absent.
fn parse_dimension(dim: Node) -> Option<(i64, i64)> {
    match (i64_child(dim, "naxis1"), i64_child(dim, "naxis2")) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

/// Trimmed, non-empty text of each `item` child under a `container` child of
/// `parent` (e.g. `inputs/planeURI`, `states/state`). Empty when absent.
fn text_value_list(parent: Node, container: &str, item: &str) -> Vec<String> {
    match child(parent, container) {
        Some(c) => children(c, item)
            .into_iter()
            .map(|n| node_text(n).trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        None => Vec::new(),
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

fn i64_child(parent: Node, name: &str) -> Option<i64> {
    text_child(parent, name)?.parse::<i64>().ok()
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
  <caom2:metaRelease>2012-03-14T00:00:00.000</caom2:metaRelease>
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
    <caom2:ambientTemp>2.5</caom2:ambientTemp>
    <caom2:photometric>true</caom2:photometric>
  </caom2:environment>
  <caom2:planes>
    <caom2:plane>
      <caom2:productID>1234567p</caom2:productID>
      <caom2:calibrationLevel>2</caom2:calibrationLevel>
      <caom2:dataProductType>image</caom2:dataProductType>
      <caom2:creatorID>ivo://cadc.nrc.ca/CFHT?1234567/1234567p</caom2:creatorID>
      <caom2:metaRelease>2012-03-14T00:00:00.000</caom2:metaRelease>
      <caom2:dataRelease>2013-03-14T00:00:00.000</caom2:dataRelease>
      <caom2:quality>
        <caom2:flag>good</caom2:flag>
      </caom2:quality>
      <caom2:provenance>
        <caom2:name>MegaPipe</caom2:name>
        <caom2:version>2.0</caom2:version>
        <caom2:project>CFHTLS</caom2:project>
        <caom2:producer>CADC</caom2:producer>
        <caom2:runID>run-42</caom2:runID>
        <caom2:reference>http://example.org/megapipe</caom2:reference>
        <caom2:lastExecuted>2012-04-01T09:30:00.000</caom2:lastExecuted>
        <caom2:inputs>
          <caom2:planeURI>caom:CFHT/1234566/1234566p</caom2:planeURI>
          <caom2:planeURI>caom:CFHT/1234565/1234565p</caom2:planeURI>
        </caom2:inputs>
      </caom2:provenance>
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
        <caom2:dimension>
          <caom2:naxis1>2048</caom2:naxis1>
          <caom2:naxis2>4096</caom2:naxis2>
        </caom2:dimension>
        <caom2:resolution>0.6</caom2:resolution>
        <caom2:sampleSize>0.187</caom2:sampleSize>
      </caom2:position>
      <caom2:energy>
        <caom2:bounds>
          <caom2:lower>3.5e-7</caom2:lower>
          <caom2:upper>6.0e-7</caom2:upper>
        </caom2:bounds>
        <caom2:bandpassName>r.MP9601</caom2:bandpassName>
        <caom2:emBand>Optical</caom2:emBand>
        <caom2:resolvingPower>4.5</caom2:resolvingPower>
        <caom2:restwav>4.75e-7</caom2:restwav>
      </caom2:energy>
      <caom2:time>
        <caom2:bounds>
          <caom2:lower>56000.0</caom2:lower>
          <caom2:upper>56000.01</caom2:upper>
        </caom2:bounds>
        <caom2:exposure>615.0</caom2:exposure>
      </caom2:time>
      <caom2:polarization>
        <caom2:states>
          <caom2:state>I</caom2:state>
          <caom2:state>Q</caom2:state>
        </caom2:states>
      </caom2:polarization>
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

        assert_eq!(
            obs.instrument.and_then(|i| i.name).as_deref(),
            Some("MegaPrime")
        );
        assert_eq!(obs.environment.and_then(|e| e.seeing), Some(0.7));

        assert_eq!(obs.planes.len(), 1);
        let plane = &obs.planes[0];
        assert_eq!(plane.product_id, "1234567p");
        assert_eq!(plane.calibration_level, Some(2));
        assert_eq!(plane.data_product_type.as_deref(), Some("image"));
        assert_eq!(plane.quality.as_deref(), Some("good"));
        // The release dates are the citation handle for a proprietary-period
        // observation: `dataRelease` says when the data itself became public.
        assert_eq!(
            plane.data_release.as_deref(),
            Some("2013-03-14T00:00:00.000")
        );
        assert_eq!(
            plane.meta_release.as_deref(),
            Some("2012-03-14T00:00:00.000")
        );
        assert_eq!(
            plane.creator_id.as_deref(),
            Some("ivo://cadc.nrc.ca/CFHT?1234567/1234567p")
        );
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
    fn parses_extended_detail_fields() {
        let obs = parse(SAMPLE).expect("parse ok");

        // Observation meta-release (raw ISO text) + environment extras.
        assert_eq!(obs.meta_release.as_deref(), Some("2012-03-14T00:00:00.000"));
        let env = obs.environment.expect("environment");
        assert_eq!(env.ambient_temp, Some(2.5));
        assert_eq!(env.photometric, Some(true));

        let plane = &obs.planes[0];

        // Provenance pipeline + upstream inputs.
        let pv = plane.provenance.as_ref().expect("provenance");
        assert_eq!(pv.name.as_deref(), Some("MegaPipe"));
        assert_eq!(pv.version.as_deref(), Some("2.0"));
        assert_eq!(pv.project.as_deref(), Some("CFHTLS"));
        assert_eq!(pv.producer.as_deref(), Some("CADC"));
        assert_eq!(pv.run_id.as_deref(), Some("run-42"));
        assert_eq!(pv.reference.as_deref(), Some("http://example.org/megapipe"));
        assert_eq!(pv.last_executed.as_deref(), Some("2012-04-01T09:30:00.000"));
        assert_eq!(
            pv.inputs,
            vec!["caom:CFHT/1234566/1234566p", "caom:CFHT/1234565/1234565p"]
        );

        // Position detail (dimension / resolution / sample size).
        assert_eq!(plane.position_dimension, Some((2048, 4096)));
        assert_eq!(plane.position_resolution, Some(0.6));
        assert_eq!(plane.position_sample_size, Some(0.187));

        // Energy detail (bandpass / band / resolving power / rest wavelength).
        assert_eq!(plane.energy_bandpass.as_deref(), Some("r.MP9601"));
        assert_eq!(plane.energy_em_band.as_deref(), Some("Optical"));
        assert_eq!(plane.energy_resolving_power, Some(4.5));
        assert_eq!(plane.energy_rest_wav, Some(4.75e-7));

        // Time exposure + polarization states.
        assert_eq!(plane.time_exposure, Some(615.0));
        assert_eq!(plane.polarization_states, vec!["I", "Q"]);
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
