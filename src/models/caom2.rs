//! Domain model for the CAOM-2 observation document returned by
//! `caom2ops/meta?ID=caom:{collection}/{observationID}`.
//!
//! Port of `Models/Caom2/CAOM2Observation.cs` in CanfarDesktop, trimmed to the
//! fields the Research / observation-detail viewer actually renders. The parser
//! ignores unknown elements, so additive CAOM2 schema changes never break it.

/// A parsed CAOM-2 observation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CAOM2Observation {
    pub collection: String,
    pub observation_id: String,
    /// e.g. "OBJECT" / "DARK".
    pub observation_type: Option<String>,
    /// "science" | "calibration".
    pub intent: Option<String>,
    pub sequence_number: Option<String>,
    /// Metadata-release timestamp, kept as the raw ISO-8601 text (rendered as a
    /// `YYYY-MM-DD` date). `None` when absent.
    pub meta_release: Option<String>,
    /// "exposure" / "coadd" / ...
    pub algorithm: Option<String>,

    pub proposal: Option<Caom2Proposal>,
    pub target: Option<Caom2Target>,
    pub telescope: Option<Caom2Telescope>,
    pub instrument: Option<Caom2Instrument>,
    pub environment: Option<Caom2Environment>,

    pub planes: Vec<Caom2Plane>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Caom2Proposal {
    pub id: Option<String>,
    pub pi: Option<String>,
    pub project: Option<String>,
    pub title: Option<String>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Caom2Target {
    pub name: Option<String>,
    /// CAOM2 target `<type>` (renamed `kind` to avoid the Rust keyword).
    pub kind: Option<String>,
    pub standard: Option<bool>,
    pub redshift: Option<f64>,
    pub moving: Option<bool>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Caom2Telescope {
    pub name: Option<String>,
    /// Geocentric ITRF position (x, y, z) in metres.
    pub geo_location: Option<(f64, f64, f64)>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Caom2Instrument {
    pub name: Option<String>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Caom2Environment {
    pub seeing: Option<f64>,
    pub humidity: Option<f64>,
    pub elevation: Option<f64>,
    pub tau: Option<f64>,
    /// Ambient temperature (°C).
    pub ambient_temp: Option<f64>,
    pub photometric: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Caom2Plane {
    pub product_id: String,
    pub calibration_level: Option<i32>,
    /// image / spectrum / cube / ...
    pub data_product_type: Option<String>,
    /// junk / good / ...
    pub quality: Option<String>,
    /// Who produced this plane (`ivo://…` creator id).
    pub creator_id: Option<String>,
    /// When the plane's METADATA became public, raw ISO-8601 text.
    pub meta_release: Option<String>,
    /// When the plane's DATA becomes public, raw ISO-8601 text.
    ///
    /// Part of citing a proprietary-period observation, and the field
    /// `DownloadedObservation::data_release` is filled from when an observation
    /// is saved from the detail page.
    pub data_release: Option<String>,
    /// Data-processing provenance (pipeline + upstream plane inputs).
    pub provenance: Option<Caom2Provenance>,
    /// Footprint polygon vertices as (RA, Dec) in degrees.
    pub position_bounds: Vec<(f64, f64)>,
    /// Pixel dimensions `(naxis1, naxis2)` when reported.
    pub position_dimension: Option<(i64, i64)>,
    /// Spatial resolution in arcseconds.
    pub position_resolution: Option<f64>,
    /// Pixel sample size in arcseconds.
    pub position_sample_size: Option<f64>,
    /// Spectral coverage in metres (CAOM2 native).
    pub energy_lower: Option<f64>,
    pub energy_upper: Option<f64>,
    pub energy_bandpass: Option<String>,
    pub energy_em_band: Option<String>,
    pub energy_resolving_power: Option<f64>,
    /// Rest wavelength in metres.
    pub energy_rest_wav: Option<f64>,
    /// Temporal coverage in MJD.
    pub time_lower: Option<f64>,
    pub time_upper: Option<f64>,
    /// Total exposure time in seconds.
    pub time_exposure: Option<f64>,
    /// Stokes polarization states present (free-form: "I", "Q", "RR", …).
    pub polarization_states: Vec<String>,
    pub artifacts: Vec<Caom2Artifact>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Caom2Provenance {
    pub name: Option<String>,
    pub version: Option<String>,
    pub project: Option<String>,
    pub producer: Option<String>,
    pub run_id: Option<String>,
    pub reference: Option<String>,
    /// Pipeline last-executed timestamp, kept as raw ISO-8601 text.
    pub last_executed: Option<String>,
    /// Plane URIs of the upstream observations that fed this plane.
    pub inputs: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Caom2Artifact {
    pub uri: String,
    /// science / weight / preview / aux.
    pub product_type: Option<String>,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
}
