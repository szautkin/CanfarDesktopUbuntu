//! Normalises a publisher ID to the canonical CAOM2 observation URI
//! (`caom:{collection}/{observationID}`) expected by the metadata service.
//!
//! Port of `Helpers/Caom2Uri.cs`. Accepts the shapes TAP / search return:
//!   * Observation form:   `ivo://cadc.nrc.ca/JWST?jw01147`
//!   * Plane form:         `ivo://cadc.nrc.ca/CFHT?729989/729989p` (productID stripped)
//!   * Mirror collections: `ivo://cadc.nrc.ca/JWST/mirror?jw01147` (mirror segment dropped)
//!   * Already-canonical:  `caom:CFHT/22803`
//!
//! Returns `None` if the input is not a recognisable publisher URI.

/// Map an ivo publisherID / `collection/obsID` to `caom:{collection}/{obsID}`.
pub fn to_observation_uri(publisher_id: &str) -> Option<String> {
    let trimmed = publisher_id.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Already-canonical caom:Collection/ObservationID form.
    if trimmed
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("caom:"))
    {
        let body = &trimmed[5..];
        let mut parts = body.splitn(2, '/');
        let collection = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        if collection.is_empty() || rest.is_empty() {
            return None;
        }
        // Strip any trailing /productID.
        let observation_id = rest.split('/').next().unwrap_or("");
        if observation_id.is_empty() {
            return None;
        }
        return Some(format!("caom:{collection}/{observation_id}"));
    }

    // ivo://authority/collection[/mirror]?observationID[/productID]
    if !trimmed
        .get(..6)
        .is_some_and(|p| p.eq_ignore_ascii_case("ivo://"))
    {
        return None;
    }
    let rest = &trimmed[6..];

    let (path_part, query) = match rest.find('?') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };

    // First path segment is the authority/host; the collection is the first
    // following non-"mirror" segment.
    let mut segments = path_part.split('/').filter(|s| !s.is_empty());
    let _host = segments.next();
    let collection = segments.find(|s| !s.eq_ignore_ascii_case("mirror"));
    let collection = match collection {
        Some(c) if !c.is_empty() => c,
        _ => return None,
    };

    let observation = query.split('/').next().unwrap_or("");
    if observation.is_empty() {
        return None;
    }
    Some(format!("caom:{collection}/{observation}"))
}

/// A stable local record id derived from a publisher DID.
///
/// The Research library keys records by this rather than by the raw DID, which
/// contains characters no filesystem wants. It MUST be deterministic: the same
/// observation downloaded twice has to resolve to the same record and the same
/// managed directory, or the second download silently orphans the first.
///
/// Lived in duplicate in two UI files; a divergence between them would have
/// split one observation into two library entries.
pub fn uuid_from_publisher_id(publisher_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    publisher_id.hash(&mut hasher);
    format!("obs-{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_form() {
        assert_eq!(
            to_observation_uri("ivo://cadc.nrc.ca/JWST?jw01147"),
            Some("caom:JWST/jw01147".to_string())
        );
    }

    #[test]
    fn plane_form_strips_product_id() {
        assert_eq!(
            to_observation_uri("ivo://cadc.nrc.ca/CFHT?729989/729989p"),
            Some("caom:CFHT/729989".to_string())
        );
    }

    #[test]
    fn mirror_segment_dropped() {
        assert_eq!(
            to_observation_uri("ivo://cadc.nrc.ca/JWST/mirror?jw01147"),
            Some("caom:JWST/jw01147".to_string())
        );
    }

    #[test]
    fn already_canonical() {
        assert_eq!(
            to_observation_uri("caom:CFHT/22803"),
            Some("caom:CFHT/22803".to_string())
        );
    }

    #[test]
    fn canonical_strips_trailing_product_id() {
        assert_eq!(
            to_observation_uri("caom:CFHT/22803/22803p"),
            Some("caom:CFHT/22803".to_string())
        );
    }

    #[test]
    fn case_insensitive_scheme() {
        assert_eq!(
            to_observation_uri("IVO://cadc.nrc.ca/CFHT?123"),
            Some("caom:CFHT/123".to_string())
        );
        assert_eq!(
            to_observation_uri("CAOM:CFHT/123"),
            Some("caom:CFHT/123".to_string())
        );
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(
            to_observation_uri("  caom:CFHT/1  "),
            Some("caom:CFHT/1".to_string())
        );
    }

    #[test]
    fn rejects_junk() {
        assert_eq!(to_observation_uri(""), None);
        assert_eq!(to_observation_uri("   "), None);
        assert_eq!(to_observation_uri("http://example.com/x"), None);
        // No query → no observationID.
        assert_eq!(to_observation_uri("ivo://cadc.nrc.ca/CFHT"), None);
        // caom: with no observationID.
        assert_eq!(to_observation_uri("caom:CFHT"), None);
        assert_eq!(to_observation_uri("caom:CFHT/"), None);
    }
}
