//! What the TAP service's tables actually contain.
//!
//! An agent writing ADQL had table names and one join, both from a sentence in
//! a tool description, and had to guess every column. The service knows: every
//! IVOA TAP endpoint publishes `TAP_SCHEMA`, and CADC's carries real prose —
//! `caom2.Plane.calibrationLevel` describes itself as "IVOA ObsCore calibration
//! level + extensions (-1,0,1,2,3,4)", and `TAP_SCHEMA.keys` says in words that
//! `caom2.Plane` → `caom2.Observation` is "the standard way to join" them.
//!
//! Read live rather than baked in, because the model moves: several Plane
//! columns are marked "new in 2.4" and a constant compiled last year describes
//! last year's archive. Cached because it moves on RELEASE timescales — the
//! whole schema is 401 columns and about 34 KB, fetched in under a second, and
//! re-fetching that per tool call would be silly.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::services::tap_service::TAPService;

/// How long a fetched schema is trusted.
///
/// The archive's column set changes when CADC deploys a new CAOM version, not
/// while an agent is working. An hour keeps a long session on one fetch and
/// still picks up a deployment without a restart.
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);

/// Rows to allow from a TAP_SCHEMA query.
///
/// The real answer is ~400 columns across 21 tables. The cap is a guard against
/// a service that answers with something enormous, not a limit anyone should
/// reach; exceeding it would silently truncate the schema, so it is generous.
const MAX_SCHEMA_ROWS: u32 = 10_000;

/// One column of one table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapColumn {
    pub name: String,
    pub datatype: String,
    pub description: String,
    /// Physical unit, when the archive declares one (e.g. `deg`, `d`).
    pub unit: String,
    /// IVOA Unified Content Descriptor — what the number MEANS, independent of
    /// its name. `pos.eq.ra` identifies a right ascension whatever the column
    /// is called.
    pub ucd: String,
}

/// One table, with the columns it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapTable {
    pub name: String,
    pub description: String,
    pub columns: Vec<TapColumn>,
}

/// A declared join between two tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapKey {
    pub from_table: String,
    pub target_table: String,
    pub from_column: String,
    pub target_column: String,
    pub description: String,
}

/// Everything the service says about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TapSchema {
    pub tables: Vec<TapTable>,
    pub keys: Vec<TapKey>,
}

impl TapSchema {
    /// One table by name, matched case-insensitively.
    ///
    /// ADQL identifiers are case-insensitive unless quoted, and an agent that
    /// read `caom2.Plane` from a listing may still type `caom2.plane`.
    pub fn table(&self, name: &str) -> Option<&TapTable> {
        self.tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// Every declared join that touches `table`, in either direction.
    pub fn keys_touching(&self, table: &str) -> Vec<&TapKey> {
        self.keys
            .iter()
            .filter(|k| {
                k.from_table.eq_ignore_ascii_case(table)
                    || k.target_table.eq_ignore_ascii_case(table)
            })
            .collect()
    }
}

/// Fetches and caches the TAP service's own schema.
pub struct TapSchemaService {
    tap: Arc<TAPService>,
    cache: RwLock<Option<(Instant, Arc<TapSchema>)>>,
}

impl TapSchemaService {
    pub fn new(tap: Arc<TAPService>) -> Self {
        TapSchemaService {
            tap,
            cache: RwLock::new(None),
        }
    }

    /// The schema, fetched on first use and re-used until [`CACHE_TTL`].
    pub async fn schema(&self) -> Result<Arc<TapSchema>, String> {
        if let Some(fresh) = self.cached() {
            return Ok(fresh);
        }

        let schema = Arc::new(self.fetch().await?);
        // Last writer wins. Two callers racing the first use both fetch, which
        // costs one extra request and no correctness — cheaper than holding a
        // lock across the network.
        *self.cache.write().unwrap() = Some((Instant::now(), Arc::clone(&schema)));
        Ok(schema)
    }

    /// Drop the cached copy, so the next call re-reads the service.
    pub fn invalidate(&self) {
        *self.cache.write().unwrap() = None;
    }

    /// The cached schema, or `None` — never a fetch.
    ///
    /// Public so the ADQL editor can check a query as it is typed: a check that
    /// awaited a network round trip would be a check that runs on every
    /// keystroke, and one that blocked the main thread would be worse still.
    /// `None` means "not known yet", which the checker treats as "say nothing".
    pub fn cached(&self) -> Option<Arc<TapSchema>> {
        let guard = self.cache.read().unwrap();
        let (at, schema) = guard.as_ref()?;
        (at.elapsed() < CACHE_TTL).then(|| Arc::clone(schema))
    }

    async fn fetch(&self) -> Result<TapSchema, String> {
        // Three queries rather than one join: TAP_SCHEMA is small, and a table
        // with no columns (or a service that omits `keys`) should still yield
        // the parts that did answer.
        let tables = self
            .query("SELECT table_name, description FROM TAP_SCHEMA.tables ORDER BY table_name")
            .await?;
        let columns = self
            .query(
                "SELECT table_name, column_name, datatype, description, unit, ucd \
                 FROM TAP_SCHEMA.columns ORDER BY table_name, column_name",
            )
            .await?;
        let keys = self
            .query(
                "SELECT k.from_table, k.target_table, kc.from_column, kc.target_column, \
                 k.description FROM TAP_SCHEMA.keys AS k \
                 JOIN TAP_SCHEMA.key_columns AS kc ON k.key_id = kc.key_id",
            )
            .await?;

        Ok(build_schema(&tables, &columns, &keys))
    }

    async fn query(
        &self,
        adql: &str,
    ) -> Result<crate::models::search_result::SearchResults, String> {
        self.tap
            .execute_query(adql, MAX_SCHEMA_ROWS, None)
            .await
            .map_err(|e| format!("TAP_SCHEMA query failed: {e}"))
    }
}

/// Assemble the three result sets into one schema.
///
/// Free of I/O so the assembly can be tested against real captured rows rather
/// than only against a live service.
pub fn build_schema(
    tables: &crate::models::search_result::SearchResults,
    columns: &crate::models::search_result::SearchResults,
    keys: &crate::models::search_result::SearchResults,
) -> TapSchema {
    let mut out = TapSchema::default();

    for row in rows_of(tables, &["table_name", "description"]) {
        out.tables.push(TapTable {
            name: row[0].clone(),
            description: row[1].clone(),
            columns: Vec::new(),
        });
    }

    for row in rows_of(
        columns,
        &[
            "table_name",
            "column_name",
            "datatype",
            "description",
            "unit",
            "ucd",
        ],
    ) {
        let column = TapColumn {
            name: row[1].clone(),
            datatype: row[2].clone(),
            description: row[3].clone(),
            unit: row[4].clone(),
            ucd: row[5].clone(),
        };
        match out.tables.iter_mut().find(|t| t.name == row[0]) {
            Some(table) => table.columns.push(column),
            // A column whose table the `tables` query did not list still
            // belongs to something; dropping it would hide it entirely.
            None => out.tables.push(TapTable {
                name: row[0].clone(),
                description: String::new(),
                columns: vec![column],
            }),
        }
    }

    for row in rows_of(
        keys,
        &[
            "from_table",
            "target_table",
            "from_column",
            "target_column",
            "description",
        ],
    ) {
        out.keys.push(TapKey {
            from_table: row[0].clone(),
            target_table: row[1].clone(),
            from_column: row[2].clone(),
            target_column: row[3].clone(),
            description: row[4].clone(),
        });
    }

    out
}

/// Rows projected onto `wanted`, by column NAME.
///
/// `SearchResultRow` is already a name→value map, so this cannot transpose two
/// fields the way reading by position would the day a service returned its
/// columns in another order. A name the response does not carry yields an empty
/// string: a service that omits `ucd` costs that field, not the whole schema.
fn rows_of(
    results: &crate::models::search_result::SearchResults,
    wanted: &[&str],
) -> Vec<Vec<String>> {
    results
        .rows
        .iter()
        .map(|row| wanted.iter().map(|name| field(row, name)).collect())
        .collect()
}

/// One field, exact match first and case-insensitively after.
///
/// TAP services differ on the case they echo back for `TAP_SCHEMA` columns.
fn field(row: &crate::models::search_result::SearchResultRow, name: &str) -> String {
    let exact = row.get(name);
    if !exact.is_empty() {
        return exact.to_string();
    }
    row.values
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::search_result::parse_csv;

    /// Rows as CADC actually returns them, captured 2026-08-21.
    ///
    /// Real rows rather than invented ones: the descriptions carry commas and
    /// quotes, which is exactly where a hand-rolled parser goes wrong, and the
    /// captured `calibrationLevel` row has both.
    fn captured_columns() -> crate::models::search_result::SearchResults {
        parse_csv(
            "table_name,column_name,datatype,description,unit,ucd\n             caom2.Plane,calibrationLevel,int,\"IVOA ObsCore calibration level + extensions (-1,0,1,2,3,4)\",,\n             caom2.Plane,position_bounds,clob,coverage on the sky,,pos.outline;obs.field\n             caom2.Plane,time_exposure,double,actual exposure time,d,time.duration;obs.exposure\n             caom2.Observation,target_name,char,target name,,meta.id;src\n",
        )
    }

    fn captured_tables() -> crate::models::search_result::SearchResults {
        parse_csv(
            "table_name,description\n             caom2.Observation,telescope observations\n             caom2.Plane,\"data products, with calibration level\"\n",
        )
    }

    fn captured_keys() -> crate::models::search_result::SearchResults {
        parse_csv(
            "from_table,target_table,from_column,target_column,description\n             caom2.Plane,caom2.Observation,obsID,obsID,standard way to join the caom2.Observation and caom2.Plane tables\n",
        )
    }

    #[test]
    fn columns_land_under_their_table_with_prose_intact() {
        let schema = build_schema(&captured_tables(), &captured_columns(), &captured_keys());

        let plane = schema.table("caom2.Plane").expect("caom2.Plane");
        assert_eq!(plane.columns.len(), 3);
        assert_eq!(plane.description, "data products, with calibration level");

        let cal = plane
            .columns
            .iter()
            .find(|c| c.name == "calibrationLevel")
            .expect("calibrationLevel");
        // The commas inside the quoted description must survive; splitting on
        // commas would leave "IVOA ObsCore calibration level + extensions (-1".
        assert_eq!(
            cal.description,
            "IVOA ObsCore calibration level + extensions (-1,0,1,2,3,4)"
        );
        assert_eq!(cal.datatype, "int");

        // Units and UCDs come through where the archive declares them.
        let exposure = plane
            .columns
            .iter()
            .find(|c| c.name == "time_exposure")
            .expect("time_exposure");
        assert_eq!(exposure.unit, "d");
        assert_eq!(exposure.ucd, "time.duration;obs.exposure");
    }

    /// ADQL identifiers are case-insensitive unless quoted.
    #[test]
    fn a_table_resolves_whatever_case_it_is_asked_for() {
        let schema = build_schema(&captured_tables(), &captured_columns(), &captured_keys());
        assert!(schema.table("caom2.plane").is_some());
        assert!(schema.table("CAOM2.PLANE").is_some());
        assert!(schema.table("caom2.NoSuchTable").is_none());
    }

    /// The join is reported for BOTH tables it connects.
    ///
    /// An agent describing `caom2.Observation` needs to be told it joins Plane
    /// just as much as one describing `caom2.Plane` does; the row only names
    /// the direction it was declared in.
    #[test]
    fn a_join_is_visible_from_either_end() {
        let schema = build_schema(&captured_tables(), &captured_columns(), &captured_keys());
        assert_eq!(schema.keys_touching("caom2.Plane").len(), 1);
        assert_eq!(schema.keys_touching("caom2.Observation").len(), 1);
        assert!(schema.keys_touching("caom2.Artifact").is_empty());
    }

    /// A column whose table was not listed still appears.
    ///
    /// Dropping it would hide the column entirely, and the two queries are
    /// separate requests that can disagree.
    #[test]
    fn a_column_from_an_unlisted_table_is_not_discarded() {
        let orphan = parse_csv(
            "table_name,column_name,datatype,description,unit,ucd\n             caom2.Ghost,ghostID,char,unlisted,,\n",
        );
        let schema = build_schema(
            &crate::models::search_result::SearchResults::default(),
            &orphan,
            &crate::models::search_result::SearchResults::default(),
        );
        let ghost = schema.table("caom2.Ghost").expect("the orphan table");
        assert_eq!(ghost.columns.len(), 1);
    }

    /// A field the service omits costs that field, not the schema.
    #[test]
    fn a_missing_optional_column_is_empty_rather_than_fatal() {
        // No `ucd` column at all — some services do not publish it.
        let no_ucd = parse_csv(
            "table_name,column_name,datatype,description,unit\n             caom2.Plane,energy_bounds,double,spectral coverage,m\n",
        );
        let schema = build_schema(
            &captured_tables(),
            &no_ucd,
            &crate::models::search_result::SearchResults::default(),
        );
        let col = &schema.table("caom2.Plane").expect("plane").columns[0];
        assert_eq!(col.unit, "m");
        assert_eq!(col.ucd, "", "a missing ucd must be empty, not a panic");
    }
}
