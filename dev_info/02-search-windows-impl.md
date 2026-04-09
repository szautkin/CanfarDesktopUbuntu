# Windows CanfarDesktop Search Implementation Reference (v1.1.0)

Complete implementation specification for porting to Rust/GTK4. Every class, field, method, algorithm, SQL template, and layout detail is documented.

---

## Table of Contents

1. [Models](#1-models)
2. [Services](#2-services)
3. [Helpers](#3-helpers)
4. [ViewModel](#4-viewmodel)
5. [Views (XAML + Code-Behind)](#5-views)

---

## 1. Models

### 1.1 SearchFormState

**Namespace:** `CanfarDesktop.Models`
**File:** `Models/SearchModels.cs`

Holds the complete state of the search form. Used for ADQL building and persisting recent searches.

| Property | Type | Default |
|---|---|---|
| `ObservationId` | `string` | `""` |
| `ProposalPi` | `string` | `""` |
| `ProposalId` | `string` | `""` |
| `ProposalTitle` | `string` | `""` |
| `ProposalKeywords` | `string` | `""` |
| `Intent` | `string` | `""` (valid: `""`, `"science"`, `"calibration"`) |
| `PublicOnly` | `bool` | `false` |
| `Target` | `string` | `""` |
| `ResolverService` | `string` | `"ALL"` |
| `ResolvedRA` | `double?` | `null` |
| `ResolvedDec` | `double?` | `null` |
| `SearchRadius` | `double` | `0.0167` |
| `PixelScale` | `string` | `""` |
| `PixelScaleUnit` | `string` | `"arcsec"` |
| `SpatialCutout` | `bool` | `false` |
| `ObservationDate` | `string` | `""` (range syntax: `"2020..2021"`, `"> 2019"`) |
| `DatePreset` | `string` | `""` (valid: `""`, `"Last24h"`, `"LastWeek"`, `"LastMonth"`) |
| `DateStart` | `string` | `""` (legacy simple date) |
| `DateEnd` | `string` | `""` (legacy simple date) |
| `IntegrationTimeMin` | `string` | `""` |
| `IntegrationTimeMax` | `string` | `""` |
| `IntegrationTimeUnit` | `string` | `"s"` |
| `TimeSpan` | `string` | `""` (range syntax for time bounds width) |
| `TimeSpanUnit` | `string` | `"d"` |
| `DataRelease` | `string` | `""` |
| `WavelengthMin` | `string` | `""` (legacy) |
| `WavelengthMax` | `string` | `""` (legacy) |
| `SpectralCoverage` | `string` | `""` (range syntax with overlap) |
| `SpectralCoverageUnit` | `string` | `"nm"` |
| `SpectralSampling` | `string` | `""` |
| `SpectralSamplingUnit` | `string` | `"nm"` |
| `ResolvingPower` | `string` | `""` (dimensionless range) |
| `BandpassWidth` | `string` | `""` |
| `BandpassWidthUnit` | `string` | `"nm"` |
| `RestFrameEnergy` | `string` | `""` |
| `RestFrameEnergyUnit` | `string` | `"nm"` |
| `SpectralCutout` | `bool` | `false` |
| `Bands` | `string` | `""` (comma-separated) |
| `Collections` | `string` | `""` (comma-separated) |
| `Instruments` | `string` | `""` (comma-separated) |
| `Filters` | `string` | `""` (comma-separated) |
| `CalibrationLevels` | `string` | `""` (comma-separated) |
| `DataProductTypes` | `string` | `""` (comma-separated) |
| `ObservationTypes` | `string` | `""` (comma-separated) |
| `MaxRecords` | `int` | `10000` |

### 1.2 ResolverResult

**Namespace:** `CanfarDesktop.Models`

| Property | Type | Default |
|---|---|---|
| `Target` | `string` | `""` |
| `RA` | `double` | `0` |
| `Dec` | `double` | `0` |
| `CoordSys` | `string?` | `null` |
| `ObjectType` | `string?` | `null` |
| `Service` | `string?` | `null` |

### 1.3 SearchResultRow

**Namespace:** `CanfarDesktop.Models`

| Property | Type | Default |
|---|---|---|
| `Values` | `Dictionary<string, string>` | empty |

**Methods:**
- `string Get(string key)` -- returns `Values[key]` if present, else `""`.

### 1.4 SearchResults

**Namespace:** `CanfarDesktop.Models`

| Property | Type | Default |
|---|---|---|
| `Columns` | `List<string>` | `[]` |
| `Rows` | `List<SearchResultRow>` | `[]` |
| `TotalRows` | `int` (computed) | `Rows.Count` |
| `Query` | `string?` | `null` |

### 1.5 DataTrainRow

**Namespace:** `CanfarDesktop.Models`

| Property | Type | Default |
|---|---|---|
| `Band` | `string` | `""` |
| `Collection` | `string` | `""` |
| `Instrument` | `string` | `""` |
| `Filter` | `string` | `""` |
| `CalibrationLevel` | `string` | `""` |
| `DataProductType` | `string` | `""` |
| `ObservationType` | `string` | `""` |
| `IsFresh` | `bool` | `false` |

### 1.6 ResultColumnInfo

**Namespace:** `CanfarDesktop.Models`

| Property | Type | Default |
|---|---|---|
| `Key` | `string` | `""` (cleaned key for formatting/visibility) |
| `Label` | `string` | `""` (display label) |
| `Header` | `string` | `""` (original CSV header, used as `row.Values` key) |
| `Visible` | `bool` | `false` |
| `Width` | `int` | `80` |

### 1.7 SavedQuery

**Namespace:** `CanfarDesktop.Models`

| Property | Type | Default |
|---|---|---|
| `Name` | `string` | `""` |
| `Adql` | `string` | `""` |
| `SavedAt` | `DateTime` | default |

### 1.8 RecentSearch

**Namespace:** `CanfarDesktop.Models`

| Property | Type | Default |
|---|---|---|
| `Summary` | `string` | `""` |
| `Adql` | `string` | `""` |
| `FormState` | `SearchFormState` | `new()` |
| `ResultCount` | `int` | `0` |
| `SearchedAt` | `DateTime` | default |

### 1.9 DataLinkResult

**Namespace:** `CanfarDesktop.Models`
**File:** `Models/DataLinkResult.cs`

| Property | Type | Default |
|---|---|---|
| `Thumbnails` | `List<string>` | `[]` |
| `Previews` | `List<string>` | `[]` |
| `DownloadUrl` | `string?` | `null` |
| `DirectFiles` | `List<DataLinkFile>` | `[]` |
| `DirectFileUrl` | `string?` (computed) | first `DirectFiles[0].Url` or `null` |

### 1.10 DataLinkFile

**Namespace:** `CanfarDesktop.Models`

| Property | Type | Default |
|---|---|---|
| `Url` | `string` | required |
| `ContentType` | `string` | `""` |
| `Description` | `string` | `""` |
| `Filename` | `string` (computed) | extracts filename from URL via `Uri.LocalPath`, fallback `"file"` |

---

## 2. Services

### 2.1 TAPService

**Namespace:** `CanfarDesktop.Services`
**Interface:** `ITAPService`
**File:** `Services/TAPService.cs`

#### Constructor
```
TAPService(HttpClient httpClient, ApiEndpoints endpoints)
```

#### Interface Methods

##### `Task<SearchResults> ExecuteQueryAsync(string adql, int maxRecords = 10000)`

1. POST to `endpoints.TapSyncUrl` with form-encoded body:
   - `LANG=ADQL`
   - `FORMAT=csv`
   - `MAXREC={maxRecords}`
   - `QUERY={adql}`
2. On non-success status: read body, throw `HttpRequestException` with status code and body.
3. Parse CSV response via `ParseCsv(csv, adql)`.

##### `Task<List<DataTrainRow>> GetDataTrainAsync()`

Executes the data train enumfield query with 30-second timeout:

**Exact ADQL:**
```sql
SELECT energy_emBand, collection, instrument_name,
       energy_bandpassName, calibrationLevel, dataProductType, type
FROM caom2.enumfield
ORDER BY energy_emBand, collection, instrument_name,
         energy_bandpassName, calibrationLevel, dataProductType, type
```

POST to `endpoints.TapSyncUrl` with: `LANG=ADQL`, `FORMAT=csv`, `MAXREC=50000`.
Parsed off UI thread via `Task.Run(() => ParseDataTrainCsv(csv))`.

##### `Task<ResolverResult?> ResolveTargetAsync(string target, string service = "ALL")`

GET to: `{endpoints.ResolverUrl}?target={Uri.EscapeDataString(target)}&service={service}&format=ascii&detail=max&cached=true`

Returns `null` on non-success. Parses ASCII `key=value` format:
- Splits lines by `\n`, splits each by first `=`
- Extracts `ra`, `dec` (both must parse as double)
- Also reads: `target`, `coordsys`, `oType`, `service`

#### Internal CSV Parser: `ParseCsv(string csv, string? query)`

1. Splits by `\n` (after normalizing line endings).
2. First line = column headers via `ParseCsvLine`.
3. Subsequent lines = data rows. Rows with mismatched column count are skipped (logged).
4. Each row stored as `SearchResultRow` with `Values[column[j]] = values[j]`.

#### Internal CSV Line Parser: `ParseCsvLine(string line)`

Character-by-character state machine:
- Tracks `inQuotes` boolean
- `"` toggles quote mode; `""` inside quotes = escaped literal quote
- `,` outside quotes = field separator
- Fields are trimmed
- Returns `List<string>`

#### Internal Data Train CSV Parser: `ParseDataTrainCsv(string csv)`

Lightweight specialized parser -- no intermediate `SearchResults`:
- Skips header (line 0)
- For each subsequent line, calls `ParseCsvLine`, requires >= 7 fields
- Maps positionally: `[0]=Band, [1]=Collection, [2]=Instrument, [3]=Filter, [4]=CalibrationLevel, [5]=DataProductType, [6]=ObservationType`
- Sets `IsFresh = true`

### 2.2 DataLinkService

**Namespace:** `CanfarDesktop.Services`
**File:** `Services/DataLinkService.cs`

#### Fields
- `_httpClient: HttpClient`
- `_endpoints: ApiEndpoints`
- `_cache: ConcurrentDictionary<string, DataLinkResult>`
- `_downloadSemaphore: SemaphoreSlim(3)` -- static, max 3 concurrent image downloads

#### Constructor
```
DataLinkService(HttpClient httpClient, ApiEndpoints endpoints)
```

#### URL Templates (from ApiEndpoints)

- **DataLink URL:** `{Caom2OpsBaseUrl}/datalink?id={Uri.EscapeDataString(publisherID)}&request=downloads-only`
  - Default `Caom2OpsBaseUrl`: `https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops`
- **Download URL:** `{Caom2OpsBaseUrl}/pkg?ID={Uri.EscapeDataString(publisherID)}`

#### Methods

##### `Task<DataLinkResult> GetLinksAsync(string publisherID)`

1. Return empty result if `publisherID` is blank.
2. Check `_cache` -- return if hit.
3. Send GET with `Accept: application/x-votable+xml`.
4. On non-success: cache and return empty result.
5. Parse VOTable XML via `ParseVOTable(xml)`.
6. Set `result.DownloadUrl = endpoints.DownloadUrl(publisherID)`.
7. Cache and return.
8. On exception: return empty result (NOT cached -- allows retry).

##### `string GetDownloadUrl(string publisherID)` -- delegates to `endpoints.DownloadUrl(publisherID)`.

##### `Task<HttpResponseMessage> DownloadAsync(string url, int timeoutSeconds = 30)`
- GET with `ResponseHeadersRead` completion option, timeout via CTS.
- Calls `EnsureSuccessStatusCode()`.
- Returns the response (caller must dispose).

##### `Task<byte[]?> DownloadImageBytesAsync(string url, int timeoutSeconds = 15)`

Concurrency-limited image download with retry:
1. Acquire `_downloadSemaphore` (max 3 concurrent).
2. Try up to 2 attempts:
   - GET with 15s timeout.
   - On non-success: return `null`.
   - On `HttpRequestException`/`TaskCanceledException`/`OperationCanceledException`: retry after 300ms delay (only on first attempt).
   - On `IOException`: return `null` immediately (no retry -- connection dropped by CADC under load).
3. Release semaphore in `finally`.

#### VOTable Parser: `ParseVOTable(string xml)` (internal static)

**Step 1 -- Extract FIELD names to determine column indices:**

Regex: `<FIELD[^>]*name="([^"]*)"\s*[^>]*/?>` (IgnoreCase)

Builds ordered list of field names. Looks up indices for:
- `access_url`
- `semantics`
- `content_type`
- `description`
- `error_message`

Returns empty if `access_url` or `semantics` index < 0.

**Step 2 -- Extract rows:**

Regex: `<TR>(.*?)</TR>` (Singleline | IgnoreCase)

For each row match, calls `ParseTDCells` on inner content.

**ParseTDCells regex:** `<TD\s*/\s*>|<TD>(.*?)</TD>` (Singleline | IgnoreCase)
- Self-closing `<TD/>` or `<TD />` = empty string
- `<TD>content</TD>` = trimmed content

**Step 3 -- Classify by semantics:**

For each row:
- Skip if fewer cells than max needed index.
- Skip if `error_message` cell is non-blank.
- Skip if `access_url` cell is blank.
- Classification:
  - `semantics == "#thumbnail"` --> add URL to `result.Thumbnails`
  - `semantics == "#preview"` AND `content_type` contains `"image"` (case-insensitive) --> add URL to `result.Previews`
  - `semantics == "#this"` --> add as `DataLinkFile` with URL, ContentType, Description

#### Caching Strategy
- Successful responses (even empty): cached in `ConcurrentDictionary` by publisherID.
- Failures (exceptions): NOT cached, allowing retry.
- No TTL/expiration -- cache lives for the lifetime of the service instance.

### 2.3 SearchStoreService

**Namespace:** `CanfarDesktop.Services`
**Interface:** `ISearchStoreService`
**File:** `Services/SearchStoreService.cs`

#### Constants
- `MaxRecentSearches = 20`
- `RecentFile = "recent_searches.json"`
- `SavedFile = "saved_queries.json"`

#### Storage Paths
- `_recentPath = Path.Combine(ApplicationData.Current.LocalFolder.Path, "recent_searches.json")`
- `_savedPath = Path.Combine(ApplicationData.Current.LocalFolder.Path, "saved_queries.json")`
- If `ApplicationData.Current` fails (unpackaged): both paths are `null`, all operations are no-ops.

For the Rust/GTK4 port, use `XDG_DATA_HOME/verbinal/` or equivalent.

#### Interface Methods

##### `List<RecentSearch> LoadRecentSearches()`
- Read file, JSON deserialize to `List<RecentSearch>`. Return `[]` on failure.

##### `void SaveRecentSearch(RecentSearch search)`
- Load existing list, insert at index 0.
- If count > 20, remove excess from end.
- Write JSON.

##### `void SaveAllRecentSearches(IEnumerable<RecentSearch> searches)`
- Write full list as JSON (used after removing individual items).

##### `void ClearRecentSearches()`
- Delete the file.

##### `List<SavedQuery> LoadSavedQueries()`
- Read file, JSON deserialize. Return `[]` on failure.

##### `void SaveQuery(SavedQuery query)`
- Load existing, remove any with same `Name`, insert at index 0.
- Write JSON.

##### `void DeleteQuery(string name)`
- Load existing, remove by `Name`, write JSON.

**JSON options:** `PropertyNameCaseInsensitive = true`.

---

## 3. Helpers

### 3.1 ADQLBuilder

**Namespace:** `CanfarDesktop.Helpers`
**File:** `Helpers/ADQLBuilder.cs`

Static class. Single public method: `string Build(SearchFormState state)`.

#### The EXACT SELECT Column List (41 columns)

```sql
SELECT TOP {MaxRecords}
    Observation.observationID,
    Observation.collection,
    Observation.sequenceNumber,
    Plane.productID,
    COORD1(CENTROID(Plane.position_bounds)) AS "RA (J2000.0)",
    COORD2(CENTROID(Plane.position_bounds)) AS "Dec. (J2000.0)",
    Observation.target_name AS "Target Name",
    Plane.time_bounds_lower AS "Start Date",
    Plane.time_exposure AS "Int. Time",
    Observation.instrument_name AS "Instrument",
    Plane.energy_bandpassName AS "Filter",
    Plane.calibrationLevel AS "Cal. Lev.",
    Observation.type AS "Obs. Type",
    Observation.proposal_id AS "Proposal ID",
    Observation.proposal_pi AS "PI Name",
    Plane.dataRelease AS "Data Release",
    Observation.observationID AS "Obs. ID",
    Plane.energy_bounds_lower AS "Min. Wavelength",
    Plane.energy_bounds_upper AS "Max. Wavelength",
    AREA(Plane.position_bounds) AS "Field of View",
    Plane.position_sampleSize AS "Pixel Scale",
    Plane.energy_resolvingPower AS "Resolving Power",
    Plane.time_bounds_upper AS "End Date",
    Plane.dataProductType AS "Data Type",
    Observation.target_moving AS "Moving Target",
    Plane.provenance_name AS "Provenance Name",
    Observation.intent AS "Intent",
    Observation.target_type AS "Target Type",
    Observation.algorithm_name AS "Algorithm",
    Observation.proposal_title AS "Proposal Title",
    Observation.proposal_keywords AS "Proposal Keywords",
    Plane.position_resolution AS "Spatial Resolution",
    Plane.energy_transition_species AS "Molecule",
    Plane.energy_transition_transition AS "Transition",
    Plane.energy_emBand AS "Band",
    Plane.energy_bounds_width AS "Bandpass Width",
    Plane.energy_sampleSize AS "Energy Sample Size",
    Plane.energy_restwav AS "Rest Frame Energy",
    Plane.time_bounds_width AS "Time Span",
    Observation.requirements_flag AS "Quality",
    Plane.publisherID
```

#### The EXACT FROM Clause

```sql
FROM caom2.Plane AS Plane JOIN caom2.Observation AS Observation ON Plane.obsID = Observation.obsID
```

#### The Quality Filter (always first WHERE clause)

```sql
( Plane.quality_flag IS NULL OR Plane.quality_flag != 'junk' )
```

#### WHERE Clause Construction

Clauses are joined with `\nAND `.

##### Observation Clauses (`AddObservationClauses`)

- **ObservationId:**
  - If contains `*`: `lower(Observation.observationID) LIKE '{value_with_*_replaced_by_%}'`
  - Else: `lower(Observation.observationID) = '{value}'`
- **ProposalPi/ProposalId/ProposalTitle/ProposalKeywords:** Each via `AddLikeClause`:
  - `lower({column}) LIKE '%{escaped_value}%'`

##### Spatial Clauses (`AddSpatialClauses`)

- **Resolved coordinates (RA + Dec present):**
  ```sql
  INTERSECTS( CIRCLE('ICRS', {RA}, {Dec}, {Radius}), Plane.position_bounds ) = 1
  ```
- **Target name only (no resolved coords):**
  ```sql
  lower(Observation.target_name) LIKE '%{target}%'
  ```
- **Pixel Scale** (if range parses): converted to degrees via `UnitConverter.TryConvertToDegrees`, then applied as range clause on `Plane.position_sampleSize`.

##### Temporal Clauses (`AddTemporalClauses`)

**Priority order:**

1. **DatePreset** (highest priority):
   - `Last24h` = now - 1 day
   - `LastWeek` = now - 7 days
   - `LastMonth` = now - 1 month
   - Converts to MJD, produces: `INTERSECTS( INTERVAL( {mjdStart}, {mjdEnd} ), Plane.time_bounds_samples ) = 1`

2. **ObservationDate** (range syntax): Dispatches to `AddDateRangeClause` which handles:
   - `Between` (A..B): `INTERSECTS( INTERVAL( {mjdLo}, {mjdHi} ), Plane.time_bounds_samples ) = 1`
   - `GreaterThan`: `Plane.time_bounds_lower > {mjd}`
   - `GreaterThanOrEqual`: `Plane.time_bounds_lower >= {mjd}`
   - `LessThan`: `Plane.time_bounds_upper < {mjd}`
   - `LessThanOrEqual`: `Plane.time_bounds_upper <= {mjd}`
   - `Equals`: Expands via `TryExpandDateToRange` (see below), then `INTERSECTS( INTERVAL( ... ) )`

3. **Legacy DateStart/DateEnd** (fallback):
   - Both: `INTERSECTS( INTERVAL( {mjdStart}, {mjdEnd} ), Plane.time_bounds_samples ) = 1`
   - Start only: `Plane.time_bounds_lower >= {mjdStart}`
   - End only: `Plane.time_bounds_upper <= {mjdEnd}`

- **IntegrationTime** (min/max with unit conversion to seconds):
  - Extracts inline unit suffix from the value string (e.g. "100m" -> value "100", unit "m")
  - Converts to seconds via `UnitConverter.TryConvertToSeconds`
  - Produces: `Plane.time_exposure >= {s}` and/or `Plane.time_exposure <= {s}`

- **TimeSpan** (range syntax, converted to days): Applied on `Plane.time_bounds_width`.

- **DataRelease** (range syntax): Dispatches to `AddDateRangeClause` with `column = "Plane.dataRelease"`.

##### Spectral Clauses (`AddSpectralClauses`)

- **SpectralCoverage** (overlap semantics via `AddSpectralOverlapClause`):
  - `Between`: Extracts inline unit suffixes from both values, converts both to metres. Produces:
    ```sql
    Plane.energy_bounds_lower <= {hi_metres} AND {lo_metres} <= Plane.energy_bounds_upper
    ```
    (This is overlap semantics: observation range intersects query range.)
  - `GreaterThan/GreaterThanOrEqual`: `{metres} <= Plane.energy_bounds_upper`
  - `LessThan/LessThanOrEqual`: `Plane.energy_bounds_lower <= {metres}`
  - `Equals`: `Plane.energy_bounds_lower <= {metres} AND {metres} <= Plane.energy_bounds_upper`

- **Legacy WavelengthMin/WavelengthMax** (fallback, if SpectralCoverage is empty):
  - Both: `INTERSECTS( INTERVAL( {min}, {max} ), Plane.energy_bounds_samples ) = 1`
  - Min only: `Plane.energy_bounds_lower >= {min}`
  - Max only: `Plane.energy_bounds_upper <= {max}`

- **SpectralSampling**: Range on `Plane.energy_sampleSize`, converted to metres.
- **ResolvingPower**: Dimensionless numeric range on `Plane.energy_resolvingPower`.
- **BandpassWidth**: Range on `Plane.energy_bounds_width`, converted to metres.
- **RestFrameEnergy**: Range on `Plane.energy_restwav`, converted to metres.

##### Data Train Clauses (`AddDataTrainClauses`)

Each via `AddInClause(column, commaSeparated, clauses)`:
- Single value: `{column} = '{value}'`
- Multiple values: `{column} IN ( '{v1}', '{v2}', ... )`
- Empty string = skipped.

| Selection | ADQL Column |
|---|---|
| Bands | `Plane.energy_emBand` |
| Collections | `Observation.collection` |
| Instruments | `Observation.instrument_name` |
| Filters | `Plane.energy_bandpassName` |
| CalibrationLevels | `Plane.calibrationLevel` |
| DataProductTypes | `Plane.dataProductType` |
| ObservationTypes | `Observation.type` |

##### Misc Clauses (`AddMiscClauses`)

- **Intent** (if non-empty): `Observation.intent = '{intent}'`
- **PublicOnly** (if true): `Plane.dataRelease <= GETDATE()`

#### Escape Functions

- `Escape(value)`: `'` -> `''`
- `EscapeLike(value)`: calls `Escape`, then `%` -> `\%`, `_` -> `\_`
- `F(double v)`: formats with `"G10"` and `CultureInfo.InvariantCulture`

#### MJD Conversion: `TryParseDateToMJD(DateTime dt, out double mjd)`

Algorithm (Julian Date formula):
```
y = dt.Year, m = dt.Month
d = dt.Day + dt.Hour/24.0 + dt.Minute/1440.0 + dt.Second/86400.0

if m <= 2: y--, m += 12

a = y / 100  (integer division)
b = 2 - a + a/4  (integer division for a/4)
JD = floor(365.25 * (y + 4716)) + floor(30.6001 * (m + 1)) + d + b - 1524.5
MJD = JD - 2400000.5
```

Overload `TryParseDateToMJD(string, out double)` parses string first with `DateTime.TryParse` using `CultureInfo.InvariantCulture`.

#### Date Expansion: `TryExpandDateToRange(string dateStr, out double mjdLo, out double mjdHi)`

- `"2020"` (4 chars, parses as year) -> `2020-01-01 00:00:00` .. `2020-12-31 23:59:59`
- `"2020-06"` (7 chars, yyyy-MM format) -> `2020-06-01 00:00:00` .. `2020-06-30 23:59:59`
- Full date or longer -> single point (mjdLo = mjdHi = parsed MJD)

### 3.2 RangeParser

**Namespace:** `CanfarDesktop.Helpers`
**File:** `Helpers/RangeParser.cs`

#### Types

```
enum RangeOperand { Equals, LessThan, LessThanOrEqual, GreaterThan, GreaterThanOrEqual, Between }
record ParsedRange(RangeOperand Operand, string Value1, string? Value2 = null)
```

#### `static bool TryParse(string? input, out ParsedRange? result)`

Returns false if input is null/whitespace.

Parse order:
1. **Range `A..B`**: Find `..` index. If both left and right non-empty: `Between(left, right)`.
2. **`<=`**: `LessThanOrEqual(value)`
3. **`>=`**: `GreaterThanOrEqual(value)`
4. **`<`**: `LessThan(value)`
5. **`>`**: `GreaterThan(value)`
6. **Plain value**: `Equals(value)`

All values are trimmed strings (NOT parsed to numbers yet -- that happens in the ADQL builder).

### 3.3 UnitConverter

**Namespace:** `CanfarDesktop.Helpers`
**File:** `Helpers/UnitConverter.cs`

#### Constants
- `SpeedOfLight = 299792458.0` m/s
- `PlanckConstant = 6.62607015e-34` J*s
- `EvToJoules = 1.602176634e-19` J/eV

#### Unit Arrays (for UI ComboBoxes)
- `SpectralUnits`: `["m", "cm", "mm", "\u00b5m", "nm", "\u00c5", "Hz", "kHz", "MHz", "GHz", "eV", "keV", "MeV", "GeV"]`
- `TimeUnits`: `["s", "m", "h", "d", "y"]`
- `PixelScaleUnits`: `["arcsec", "arcmin", "deg"]`

#### Suffix Extraction

##### `(string numeric, string? unit) ExtractSpectralSuffix(string raw)`
Regex: `(GHz|MHz|kHz|GeV|MeV|keV|nm|um|\u00b5m|mm|cm|Hz|eV|\u00c5|A|m)$` (IgnoreCase, Compiled)
Splits into numeric part + trailing unit. Returns `(raw, null)` if no match.

##### `(string numeric, string? unit) ExtractTimeSuffix(string raw)`
Regex: `([smhdy])$` (IgnoreCase, Compiled)

##### `(string numeric, string? unit) ExtractPixelScaleSuffix(string raw)`
Regex: `(arcmin|arcsec|deg)$` (IgnoreCase, Compiled)

#### Conversion: `TryConvertToMetres(string numericValue, string unit, out double metres)`

Unit string is lowercased and normalized (`\u00b5` -> `u`, `\u00c5`/`\u00e5` -> `a`).

**Wavelength -> metres (direct multiplication):**

| Unit | Factor |
|---|---|
| `m` | 1.0 |
| `cm` | 1e-2 |
| `mm` | 1e-3 |
| `um` | 1e-6 |
| `nm` | 1e-9 |
| `a` (Angstrom) | 1e-10 |

**Frequency -> metres (lambda = c / f):**

| Unit | Factor (Hz) |
|---|---|
| `hz` | 1.0 |
| `khz` | 1e3 |
| `mhz` | 1e6 |
| `ghz` | 1e9 |

Formula: `metres = SpeedOfLight / (val * factor)`

**Energy -> metres (lambda = hc / E):**

| Unit | Factor (eV) |
|---|---|
| `ev` | 1.0 |
| `kev` | 1e3 |
| `mev` | 1e6 |
| `gev` | 1e9 |

Formula: `energyJ = val * factor * EvToJoules; metres = PlanckConstant * SpeedOfLight / energyJ`

Returns false if value doesn't parse or value <= 0.

#### Conversion: `TryConvertToSeconds(string numericValue, string unit, out double seconds)`

| Unit | Factor |
|---|---|
| `s` | 1.0 |
| `m` | 60.0 |
| `h` | 3600.0 |
| `d` | 86400.0 |
| `y` | 365.25 * 86400.0 |

#### Conversion: `TryConvertToDays(string numericValue, string unit, out double days)`

| Unit | Factor |
|---|---|
| `s` | 1.0 / 86400.0 |
| `m` | 1.0 / 1440.0 |
| `h` | 1.0 / 24.0 |
| `d` | 1.0 |
| `y` | 365.25 |

#### Conversion: `TryConvertToDegrees(string numericValue, string unit, out double degrees)`

| Unit | Factor |
|---|---|
| `arcsec` | 1.0 / 3600.0 |
| `arcmin` | 1.0 / 60.0 |
| `deg` | 1.0 |

#### `bool IsInverseUnit(string unit)`
Returns true for: `hz`, `khz`, `mhz`, `ghz`, `ev`, `kev`, `mev`, `gev`.

### 3.4 CellFormatter

**Namespace:** `CanfarDesktop.Helpers`
**File:** `Helpers/CellFormatter.cs`

#### Key Cleaning: `static string CleanKey(string header)`

```
header.Replace("\"", "").Trim().ToLower(InvariantCulture).Replace(" ", "").Replace(".", "")
```

Example: `"RA (J2000.0)"` -> `ra(j20000)`, `"Cal. Lev."` -> `callev`

#### Format Dispatch: `static string Format(string columnKey, string raw)`

Trims raw value. Returns `""` if empty. Dispatches on `CleanKey(columnKey)`:

| Key Pattern | Formatter | Output Pattern |
|---|---|---|
| `startdate`, `enddate`, `provelastexecuted` | `FormatMjdDate` | `yyyy-MM-dd` |
| `ra(j20000)`, `dec(j20000)` | `FormatCoordinate(_, 5)` | 5 decimal places (`F5`) |
| `inttime` | `FormatIntegrationTime` | Adaptive units (see below) |
| `callev` | `FormatCalibrationLevel` | Map 0/1/2/3 to names |
| `download`, `movingtarget` | `FormatBoolean` | Checkmark or empty |
| `minwavelength`, `maxwavelength`, `restframeenergy` | `FormatWavelength` | Scientific or G6 |
| `pixelscale` | `FormatScientific(_, 4)` | E4 or G4 |
| `fieldofview` | `FormatScientific(_, 6)` | E6 or G6 |
| `datarelease` | `FormatTimestamp` | Cleaned ISO timestamp |
| everything else | identity | trimmed raw value |

#### FormatMjdDate

```
unixSeconds = (mjd - 40587.0) * 86400.0
dt = UnixEpoch + unixSeconds (UTC)
output = dt.ToString("yyyy-MM-dd")
```

**Key constant: MJD of Unix epoch = 40587.0**

#### FormatIntegrationTime

Input is seconds (double).

- `>= 3600`: Display in hours. If close to integer: `{int}h`, else `{F1}h`
- `>= 60`: Display in minutes. If close to integer: `{int}m`, else `{F1}m`
- Otherwise: Display in seconds. If close to integer: `{int}s`, else `{F1}s`

"Close to integer" = `abs(value - round(value)) < 0.01`

#### FormatCalibrationLevel

| Raw | Output |
|---|---|
| `"0"` | `"Raw"` |
| `"1"` | `"Cal"` |
| `"2"` | `"Product"` |
| `"3"` | `"Composite"` |
| other | raw value |

#### FormatBoolean

- `"true"` (case-insensitive) or `"1"` -> `"\u2713"` (checkmark)
- Otherwise -> `""` (empty)

#### FormatWavelength

If abs(v) < 0.001 or abs(v) > 1e6: scientific `E3`
Otherwise: `G6`

#### FormatScientific(raw, decimals)

If abs(v) < 0.001 or abs(v) > 1e6: scientific `E{decimals}`
Otherwise: `G{decimals}`

#### FormatTimestamp

If raw doesn't contain `T` or space: return as-is.
Otherwise: Replace `T` with space, remove `Z`, truncate at first `.` after position 10.

#### Default Visible Column Keys

```
HashSet<string>(OrdinalIgnoreCase):
  "download", "preview",
  "collection", "targetname", "ra(j20000)", "dec(j20000)",
  "startdate", "instrument", "filter", "callev",
  "obstype", "proposalid", "piname", "obsid"
```

#### Column Width Table

| Key | Width (px) |
|---|---|
| `download` | 35 |
| `preview` | 35 |
| `collection`, `proposalid`, `obsid` | 100 |
| `targetname`, `piname` | 110 |
| `ra(j20000)`, `dec(j20000)` | 95 |
| `startdate`, `enddate`, `datarelease` | 90 |
| `instrument` | 90 |
| `inttime` | 65 |
| `filter`, `callev`, `band` | 60 |
| `obstype`, `datatype` | 75 |
| default | 80 |

### 3.5 DataTrainManager

**Namespace:** `CanfarDesktop.Helpers`
**File:** `Helpers/DataTrainManager.cs`

Pure logic class, no UI dependencies.

#### Fields

- `_rows: List<DataTrainRow>` = `[]`

#### Properties -- All Values (computed from _rows)

- `AllBands: List<string>` = `[]`
- `AllCollections: List<string>` = `[]`
- `AllInstruments: List<string>` = `[]`
- `AllFilters: List<string>` = `[]`
- `AllCalLevels: List<string>` = `[]`
- `AllDataTypes: List<string>` = `[]`
- `AllObsTypes: List<string>` = `[]`

#### Properties -- Available (filtered by cascade)

- `AvailableBands: HashSet<string>` = `[]`
- `AvailableCollections: HashSet<string>` = `[]`
- `AvailableInstruments: HashSet<string>` = `[]`
- `AvailableFilters: HashSet<string>` = `[]`
- `AvailableCalLevels: HashSet<string>` = `[]`
- `AvailableDataTypes: HashSet<string>` = `[]`
- `AvailableObsTypes: HashSet<string>` = `[]`

#### Properties -- User Selections

- `SelectedBands: HashSet<string>` = `[]`
- `SelectedCollections: HashSet<string>` = `[]`
- `SelectedInstruments: HashSet<string>` = `[]`
- `SelectedFilters: HashSet<string>` = `[]`
- `SelectedCalLevels: HashSet<string>` = `[]`
- `SelectedDataTypes: HashSet<string>` = `[]`
- `SelectedObsTypes: HashSet<string>` = `[]`

Computed string properties for ADQL: `BandsString`, `CollectionsString`, etc. -- comma-joined from the selected sets.

#### `void Load(List<DataTrainRow> rows)`

1. Store `_rows = rows`.
2. Compute `All*` via `Distinct()` -- `SortedSet` for alphabetical order, skip blanks.
3. Call `Refresh()`.

#### `void Toggle(int columnIndex, string value)`

1. Get the selected set for `columnIndex` (0=Bands, 1=Collections, ..., 6=ObsTypes).
2. If value is in set: remove it. Otherwise: add it.
3. **Clear downstream selections** -- for all columns AFTER `columnIndex`, clear their selected set.
4. Call `Refresh()`.

#### `void ClearAll()`

Clear all 7 selected sets, then call `Refresh()`.

#### CASCADE FILTERING ALGORITHM: `void Refresh()`

This is the core algorithm:

```
AvailableBands = copy of AllBands (never filtered)

rows = _rows  (start with full set)

1. If SelectedBands is non-empty:
   rows = rows filtered to only rows where row.Band is in SelectedBands
2. AvailableCollections = distinct Collection values from filtered rows
3. Prune SelectedCollections: intersect with AvailableCollections

4. If SelectedCollections is non-empty:
   rows = rows filtered to only rows where row.Collection is in SelectedCollections
5. AvailableInstruments = distinct Instrument values from filtered rows
6. Prune SelectedInstruments: intersect with AvailableInstruments

7. If SelectedInstruments is non-empty:
   rows = rows filtered ...
8. AvailableFilters = distinct ...
9. Prune SelectedFilters ...

... (same pattern continues for CalLevels, DataTypes, ObsTypes)
```

Each step:
1. Apply current column's selection as a filter on rows (if any selected).
2. Compute available values for the NEXT column from the filtered rows.
3. Prune the next column's selections to only values that are still available.

**Column cascade order:**
Band -> Collection -> Instrument -> Filter -> CalibrationLevel -> DataProductType -> ObservationType

#### Helper Functions

- `Distinct(rows, selector)` -> `List<string>` via `SortedSet`, skipping blanks
- `DistinctSet(rows, selector)` -> `HashSet<string>`, skipping blanks
- `Filter(rows, selected, selector)` -> if selected is empty, return rows unchanged; else filter by `selected.Contains(selector(row))`
- `Prune(selected, available)` -> `selected.IntersectWith(available)`

### 3.6 FilterToAdqlConverter

**Namespace:** `CanfarDesktop.Helpers`
**File:** `Helpers/FilterToAdqlConverter.cs`

#### Column Key to ADQL Mapping

| Cleaned Key | ADQL Column |
|---|---|
| `observationid` | `Observation.observationID` |
| `collection` | `Observation.collection` |
| `targetname` | `Observation.target_name` |
| `instrument` | `Observation.instrument_name` |
| `filter` | `Plane.energy_bandpassName` |
| `callev` | `Plane.calibrationLevel` |
| `obstype` | `Observation.type` |
| `proposalid` | `Observation.proposal_id` |
| `piname` | `Observation.proposal_pi` |
| `obsid` | `Observation.observationID` |
| `datatype` | `Plane.dataProductType` |
| `band` | `Plane.energy_emBand` |
| `intent` | `Observation.intent` |
| `ra(j20000)` | `COORD1(CENTROID(Plane.position_bounds))` |
| `dec(j20000)` | `COORD2(CENTROID(Plane.position_bounds))` |
| `startdate` | `Plane.time_bounds_lower` |
| `enddate` | `Plane.time_bounds_upper` |
| `inttime` | `Plane.time_exposure` |
| `minwavelength` | `Plane.energy_bounds_lower` |
| `maxwavelength` | `Plane.energy_bounds_upper` |
| `pixelscale` | `Plane.position_sampleSize` |
| `resolvingpower` | `Plane.energy_resolvingPower` |
| `fieldofview` | `AREA(Plane.position_bounds)` |

#### `static string? ConvertFilters(IReadOnlyDictionary<string, string> filters)`

For each filter entry:
1. Skip if text is blank.
2. Clean the key via `CellFormatter.CleanKey`.
3. Look up ADQL column. Skip if not found.
4. Build clause via `BuildClause`.

Clauses joined with `\nAND `.

#### `static string AppendToQuery(string baseAdql, IReadOnlyDictionary<string, string> filters)`

Returns `"{baseAdql.TrimEnd()}\nAND {fragment}"` or original query if no filters.

#### `BuildClause(string adqlCol, string value)` (private)

1. If value parses as double: `{adqlCol} = {value_G10}`
2. Otherwise: `lower({adqlCol}) LIKE '%{escaped_lowercase_value}%'`

Escaping: `'` -> `''`, `%` -> `\%`, `_` -> `\_`

### 3.7 ResultFilter

**Namespace:** `CanfarDesktop.Helpers`
**File:** `Helpers/ResultFilter.cs`

#### `static List<SearchResultRow> Filter(IReadOnlyList<SearchResultRow> rows, IReadOnlyDictionary<string, string> filters, Func<string, string> getHeader)`

Algorithm:
1. If rows empty or no filters: return copy of rows.
2. Pre-resolve: for each filter `(key, text)`, call `getHeader(key)` to get the CSV column header. Skip blank headers.
3. Filter rows: keep only rows where ALL filters match (AND logic). Each filter checks:
   `row.Get(header).Contains(text, StringComparison.OrdinalIgnoreCase)`
4. Returns new list (does NOT mutate input).

### 3.8 ResultSorter

**Namespace:** `CanfarDesktop.Helpers`
**File:** `Helpers/ResultSorter.cs`

#### `static List<SearchResultRow> Sort(IReadOnlyList<SearchResultRow> rows, string columnHeader, bool ascending)`

1. If rows empty or columnHeader empty: return copy.
2. Copy rows to new list.
3. Sort using `SmartCompare`:
   - Both empty: `0`
   - One empty: empty sorts **last** (returns `1` if `a` empty, `-1` if `b` empty)
   - Both parse as double: numeric comparison
   - Otherwise: `string.Compare` case-insensitive
4. If descending: negate comparison result.
5. Returns new sorted list (does NOT mutate input).

---

## 4. ViewModel

### 4.1 SearchViewModel

**Namespace:** `CanfarDesktop.ViewModels`
**File:** `ViewModels/SearchViewModel.cs`
**Base class:** `ObservableObject` (CommunityToolkit.Mvvm)

#### Constructor
```
SearchViewModel(ITAPService tapService, ISearchStoreService storeService)
```

#### Fields
- `_tapService: ITAPService`
- `_storeService: ISearchStoreService`
- `_allDataTrainRows: List<DataTrainRow>` = `[]`
- `_resolverCts: CancellationTokenSource?`

#### Observable Properties

All `[ObservableProperty]` fields generate `PropertyName` getter/setter with change notification.

**Observation:** `ObservationId`, `ProposalPi`, `ProposalId`, `ProposalTitle`, `ProposalKeywords`, `Intent`, `PublicOnly`

**Spatial:** `Target`, `ResolverService` (default `"ALL"`), `ResolverStatus`, `ResolvedRA` (double?), `ResolvedDec` (double?), `SearchRadius` (0.0167), `PixelScale`, `PixelScaleUnit` (`"arcsec"`), `SpatialCutout`

**Temporal:** `ObservationDate`, `DatePreset`, `DateStart`, `DateEnd`, `IntegrationTimeMin`, `IntegrationTimeMax`, `IntegrationTimeUnit` (`"s"`), `TimeSpan`, `TimeSpanUnit` (`"d"`), `DataRelease`

**Spectral:** `WavelengthMin`, `WavelengthMax`, `SpectralCoverage`, `SpectralCoverageUnit` (`"nm"`), `SpectralSampling`, `SpectralSamplingUnit` (`"nm"`), `ResolvingPower`, `BandpassWidth`, `BandpassWidthUnit` (`"nm"`), `RestFrameEnergy`, `RestFrameEnergyUnit` (`"nm"`), `SpectralCutout`

**General:** `MaxRecords` (10000), `AdqlText`, `IsSearching`, `IsLoadingDataTrain`, `IsResolving`, `ErrorMessage`, `HasError`, `StatusMessage`, `Results` (SearchResults?)

**Pagination:** `CurrentPage` (1), `RowsPerPage` (50), `TotalPages` (1), `PageStatus`

**Non-observable:** `RowsPerPageOptions: int[]` = `[25, 50, 100, 250, 500]`

#### Observable Collections

**Data Train (ObservableCollection<string> each):**
- Available: `AvailableBands`, `AvailableCollections`, `AvailableInstruments`, `AvailableFilters`, `AvailableCalLevels`, `AvailableDataTypes`, `AvailableObsTypes`
- Selected: `SelectedBands`, `SelectedCollections`, `SelectedInstruments`, `SelectedFilters`, `SelectedCalLevels`, `SelectedDataTypes`, `SelectedObsTypes`

**Side panel:**
- `RecentSearches: ObservableCollection<RecentSearch>`
- `SavedQueries: ObservableCollection<SavedQuery>`

**Column info:**
- `ResultColumns: ObservableCollection<ResultColumnInfo>`

**Static:** `ResolverServices: string[]` = `["ALL", "SIMBAD", "NED", "VIZIER", "NONE"]`

#### Data Train Cache

**Cache file path:** `ApplicationData.Current.LocalFolder.Path + "/datatrain_cache.json"`
(For Rust: use `~/.local/share/verbinal/datatrain_cache.json` or equivalent XDG path.)

##### `async Task LoadDataTrainAsync()`

1. Set `IsLoadingDataTrain = true`.
2. Try cache first (off-thread): load JSON from cache file. If > 0 rows, store in `_allDataTrainRows` and call `RefreshDataTrainOptions()`.
3. Fire-and-forget network refresh: call `_tapService.GetDataTrainAsync()`. If > 0 rows, store and save to cache file. This runs in background -- does NOT block UI.
4. Set `IsLoadingDataTrain = false`.

##### `void RefreshDataTrainOptions()`

Cascade filter logic (duplicated from DataTrainManager but operating on `ObservableCollection`):

1. `AvailableBands` = all distinct Band values from `_allDataTrainRows`.
2. If `SelectedBands` non-empty: filter rows to matching bands.
3. `AvailableCollections` = distinct Collection from filtered rows. Prune `SelectedCollections`.
4. Continue cascading for Instruments, Filters, CalLevels, DataTypes, ObsTypes.

`SetOptions`: clears the ObservableCollection, fills from `SortedSet`.
`SetOptionsAndPrune`: same as SetOptions plus removes invalid selections (iterates backward, removes if not in values set).

#### Target Resolver

##### `partial void OnTargetChanged(string value)` (auto-called on Target change)

- If `ResolverService == "NONE"` or target is blank: clear ResolvedRA/Dec/Status, return.
- Otherwise: fire-and-forget `ResolveTargetDebouncedAsync(value)`.

##### `async Task ResolveTargetDebouncedAsync(string target)`

1. Cancel previous CTS, create new one.
2. `await Task.Delay(500, token)` -- 500ms debounce.
3. Set `IsResolving = true`, `ResolverStatus = "Resolving..."`.
4. Call `_tapService.ResolveTargetAsync(target, ResolverService)`.
5. On success: set RA/Dec, format status as `"RA: {F4}  Dec: {F4}"` + optional `"  ({ObjectType})"`.
6. On not found: clear RA/Dec, set status `"Not found"`.
7. On cancel: no-op.
8. On error: set status `"Resolver error"`.
9. Always set `IsResolving = false`.

#### Date Preset

##### `partial void OnDatePresetChanged(string value)`

Maps preset to date range string:
- `Last24h` -> `"{start:yyyy-MM-dd}..{now:yyyy-MM-dd}"`
- `LastWeek` -> same pattern
- `LastMonth` -> same pattern

Sets `ObservationDate` to the computed range string.

#### Search Execution

##### `[RelayCommand] async Task SearchAsync()`

1. Build `SearchFormState` from current VM properties.
2. Build ADQL via `ADQLBuilder.Build(state)`.
3. Set `AdqlText = adql`.
4. Call `ExecuteAdqlAsync(adql)`.
5. If results non-empty: save to recent searches with summary, reload recent searches list.

##### `[RelayCommand] async Task ExecuteAdqlAsync(string? adql = null)`

1. Use provided adql or fall back to `AdqlText`.
2. Set `IsSearching = true`, `HasError = false`, `StatusMessage = "Searching..."`.
3. Call `_tapService.ExecuteQueryAsync(query, MaxRecords)`.
4. On success: store `Results`, call `ResetFiltersAndSort()`, `BuildColumns()`, reset page to 1, `UpdatePagination()`.
5. Status message: `"{TotalRows} rows returned"` + `" (limit: {MaxRecords})"` if at limit.
6. On error: set `ErrorMessage`, `HasError = true`, status `"Search failed"`.
7. Always set `IsSearching = false`.

##### `BuildSearchSummary(SearchFormState s)` (static)

Builds summary from parts: Target, Collections, ObservationDate or DatePreset, SpectralCoverage+unit, Bands. Joined with `", "`. Default: `"General search"`.

#### Pagination + Columns

##### `void BuildColumns()`

1. Clear `ResultColumns`.
2. Add virtual columns first:
   - `download`: Key=`"download"`, Label=`"\u2B73"` (downward arrow), Header=`"__download__"`, Visible=true, Width from CellFormatter.
   - `preview`: Key=`"preview"`, Label=`"\uD83D\uDDBC"` (frame with picture), Header=`"__preview__"`, Visible=true, Width from CellFormatter.
3. For each TAP CSV column header:
   - Key = `CellFormatter.CleanKey(header)`
   - Label = header with quotes removed, trimmed
   - Header = original CSV header (for row value lookup)
   - Visible = `CellFormatter.DefaultVisibleKeys.Contains(key)`
   - Width = `CellFormatter.ColumnWidth(key)`

##### `void UpdatePagination()`

```
totalFiltered = GetProcessedRows().Count
totalAll = Results?.TotalRows ?? 0

if totalAll == 0: TotalPages = 1, PageStatus = ""
else:
  TotalPages = max(1, ceil(totalFiltered / RowsPerPage))
  Clamp CurrentPage to [1, TotalPages]
  start = (CurrentPage - 1) * RowsPerPage + 1
  end = min(CurrentPage * RowsPerPage, totalFiltered)
  PageStatus = "Showing {start}-{end} of {totalFiltered} (filtered from {totalAll})"
  -- or without "(filtered from ...)" if totalFiltered == totalAll
```

##### Sort State

Private fields: `_sortColumnKey: string?`, `_sortAscending: bool = true`.
Public: `(string? Key, bool Ascending) CurrentSort`

##### `void SortBy(string columnKey)`

- If same column: toggle ascending/descending.
- If new column: set column, ascending = true.
- Reset to page 1, invalidate filter cache.

##### Filter State

Private: `_columnFilters: Dictionary<string, string>` (case-insensitive keys), `_filteredRowsCache: List<SearchResultRow>?`.

- `void SetColumnFilter(string columnKey, string filterText)` -- add/remove from dictionary, reset page 1, invalidate cache.
- `string GetColumnFilter(string columnKey)` -- lookup, default `""`.
- `void InvalidateFilterCache()` -- set cache to null.
- `void ResetFiltersAndSort()` -- clear all filters, reset sort, clear cache.
- `bool HasActiveFilters` -- `_columnFilters.Count > 0`.

##### `string? BuildFilteredAdql()`

If no filters or no AdqlText: return null.
Delegates to `FilterToAdqlConverter.AppendToQuery(AdqlText, _columnFilters)`.

##### `List<SearchResultRow> GetProcessedRows()` (private)

1. Return cache if not null.
2. If no Results: cache and return `[]`.
3. **Filter**: If filters active, call `ResultFilter.Filter(Results.Rows, _columnFilters, GetColumnHeader)`.
4. **Sort**: If sort column set, call `ResultSorter.Sort(rows, GetColumnHeader(sortColumnKey), _sortAscending)`.
5. Cache and return.

##### `List<SearchResultRow> GetCurrentPageRows()`

```
processed = GetProcessedRows()
skip = (CurrentPage - 1) * RowsPerPage
return processed.Skip(skip).Take(RowsPerPage).ToList()
```

##### Navigation Methods

- `GoToNextPage()` -- increment if < TotalPages, UpdatePagination
- `GoToPreviousPage()` -- decrement if > 1, UpdatePagination
- `GoToFirstPage()` -- set 1, UpdatePagination
- `GoToLastPage()` -- set TotalPages, UpdatePagination

##### Column Helpers

- `string[] GetVisibleColumnKeys()` -- visible columns' keys
- `string GetColumnLabel(string key)` -- lookup by key, fallback to key
- `string GetColumnHeader(string key)` -- lookup by key, fallback to key
- `int GetColumnWidth(string key)` -- lookup by key, fallback 80
- `void ToggleColumnVisibility(string key)` -- flip the Visible flag
- `string FormatCell(string columnKey, string rawValue)` -- delegates to `CellFormatter.Format`

#### Recent Searches + Saved Queries

- `void LoadRecentSearchesFromStore()` -- clear + reload from service
- `void LoadSavedQueriesFromStore()` -- clear + reload from service
- `void LoadFromRecentSearch(RecentSearch)` -- calls `LoadFromFormState(search.FormState)`, sets `AdqlText = search.Adql`
- `void RemoveRecentSearch(RecentSearch)` -- remove from collection, save all remaining
- `void ClearAllRecentSearches()` -- clear store and collection
- `void SaveCurrentQuery(string name)` -- create `SavedQuery` with current AdqlText, save, reload
- `void LoadSavedQuery(SavedQuery)` -- set `AdqlText = query.Adql`
- `void DeleteSavedQuery(SavedQuery)` -- delete from store, reload

#### Form State

##### `SearchFormState BuildFormState()`

Maps all VM properties to a new `SearchFormState`. Data train selections are comma-joined from the `SelectedXxx` ObservableCollections.

##### `void LoadFromFormState(SearchFormState s)`

Maps all `SearchFormState` properties back to VM. Data train selections are restored via `RestoreCollection(target, csv)` which clears then splits by `,` and adds each value.

##### `void ClearForm()`

Resets all fields to defaults:
- All string properties to `""` except: `ResolverService = "ALL"`, `PixelScaleUnit = "arcsec"`, `IntegrationTimeUnit = "s"`, `TimeSpanUnit = "d"`, spectral units to `"nm"`.
- `SearchRadius = 0.0167`
- `PublicOnly = false`, `SpatialCutout = false`, `SpectralCutout = false`
- Clear all 7 Selected* collections, call `RefreshDataTrainOptions()`.

#### Export

##### `string ExportResultsCsv()`

- Header line: all column names, CSV-quoted.
- Data lines: each row's values, CSV-quoted.
- Quoting: if value contains `,`, `"`, or `\n`, wrap in `"..."` with internal `"` doubled.

##### `string ExportResultsTsv()`

- Header line: columns joined by `\t`.
- Data lines: values joined by `\t`. Tabs in values replaced with space.

---

## 5. Views

### 5.1 SearchPage.xaml -- Layout Structure

**Namespace:** `CanfarDesktop.Views`
**File:** `Views/SearchPage.xaml`

#### Root Layout

```
Grid (Padding="24", ColumnSpacing="16")
  ColumnDefinitions: [*, 260]

  // Column 0: Main content
  Grid (RowSpacing="12")
    RowDefinitions: [Auto, *]
    Row 0: TextBlock "CADC Archive Search" (TitleTextBlockStyle)
    Row 1: Pivot (x:Name="MainPivot")
      PivotItem "Search Form"
      PivotItem "Results"
      PivotItem "ADQL Editor"

  // Column 1: Side panel
  ScrollViewer
    StackPanel (Spacing="12")
      Border "Recent Searches" card
      Border "Saved Queries" card
```

#### Search Form Tab

```
PivotItem Header="Search Form"
  Grid (RowSpacing="0")
    RowDefinitions: [*, Auto]

    Row 0: ScrollViewer (VerticalScrollBarVisibility="Auto", Padding="0,8,0,0")
      StackPanel (Spacing="12")

        // 4-column constraint grid (matches CADC web layout)
        Grid (ColumnSpacing="16")
          ColumnDefinitions: [*, *, *, *]

          // Col 0: Observation
          StackPanel (Spacing="8")
            TextBlock "Observation" (BodyStrongTextBlockStyle)
            TextBox "Observation ID" -> ViewModel.ObservationId (TwoWay, PropertyChanged)
            TextBox "PI Name" -> ViewModel.ProposalPi
            TextBox "Proposal ID" -> ViewModel.ProposalId
            TextBox "Proposal Title" -> ViewModel.ProposalTitle
            TextBox "Keywords" -> ViewModel.ProposalKeywords
            TextBox "Data Release" -> ViewModel.DataRelease
            CheckBox "Public only" -> ViewModel.PublicOnly (TwoWay)
            ComboBox "Intent" -> ViewModel.Intent (items: "", "science", "calibration")

          // Col 1: Spatial
          StackPanel (Spacing="8")
            TextBlock "Spatial" (BodyStrongTextBlockStyle)
            TextBox "Target or Coordinates" -> ViewModel.Target
            ComboBox "Resolver" -> ViewModel.ResolverService (items from ViewModel.ResolverServices)
            Border (resolver progress) -> Visibility bound to ViewModel.IsResolving
              ProgressRing (14x14) + "Resolving..."
            TextBlock (resolver status) -> ViewModel.ResolverStatus (CaptionStyle, Opacity 0.7)
            NumberBox "Radius (deg)" -> ViewModel.SearchRadius (Min=0, SmallChange=0.01, Compact)
            Grid (ColumnSpacing="4")
              TextBox "Pixel Scale" -> ViewModel.PixelScale
              ComboBox (Width=90) -> ViewModel.PixelScaleUnit (items from PixelScaleUnits)
            CheckBox "Spatial cutout" -> ViewModel.SpatialCutout

          // Col 2: Temporal
          StackPanel (Spacing="8")
            TextBlock "Temporal" (BodyStrongTextBlockStyle)
            Grid (ColumnSpacing="4")
              TextBox "Observation Date" -> ViewModel.ObservationDate
              ComboBox (Width=110) date preset (items: "", "Last24h", "LastWeek", "LastMonth")
            Grid (ColumnSpacing="4")
              TextBox "Integration Time" -> ViewModel.IntegrationTimeMin
              ComboBox (Width=60) -> ViewModel.IntegrationTimeUnit (items from TimeUnits)
            Grid (ColumnSpacing="4")
              TextBox "Time Span" -> ViewModel.TimeSpan
              ComboBox (Width=60) -> ViewModel.TimeSpanUnit (items from TimeUnits)

          // Col 3: Spectral
          StackPanel (Spacing="8")
            TextBlock "Spectral" (BodyStrongTextBlockStyle)
            Grid: TextBox "Spectral Coverage" + ComboBox (W=80) SpectralCoverageUnit
            Grid: TextBox "Spectral Sampling" + ComboBox (W=80) SpectralSamplingUnit
            TextBox "Resolving Power"
            Grid: TextBox "Bandpass Width" + ComboBox (W=80) BandpassWidthUnit
            Grid: TextBox "Rest Frame Energy" + ComboBox (W=80) RestFrameEnergyUnit
            CheckBox "Spectral cutout"

        // Data Train (lazy, inside Expander)
        Expander Header="Additional Constraints" IsExpanded="False"
          Grid (ColumnSpacing="8", 7 equal columns)
            Col 0: "Band" label + ListView x:Name="BandList" (MaxHeight=180, SelectionMode=Multiple, Tag="0")
            Col 1: "Collection" + CollectionList (Tag="1")
            Col 2: "Instrument" + InstrumentList (Tag="2")
            Col 3: "Filter" + FilterList (Tag="3")
            Col 4: "Cal. Level" + CalLevelList (Tag="4")
            Col 5: "Data Type" + DataTypeList (Tag="5")
            Col 6: "Obs. Type" + ObsTypeList (Tag="6")
          All ListViews: SelectionChanged="OnTrainSelectionChanged"

    Row 1: Pinned action bar (Grid, RowSpacing="0")
      Row 0: Divider (Border, Height=1, Margin="0,8,0,12")
      Row 1: Grid (ColumnSpacing="12", Padding="0,0,0,4")
        Button (AccentStyle) "Search" with icon E721 -> OnSearchClick
        Button "Reset" -> OnClearClick
        "Max Records" label + NumberBox (Width=100, Min=1, Max=30000, Compact) -> ViewModel.MaxRecords
        ProgressRing (24x24) -> ViewModel.IsSearching
      Row 2: InfoBar (Error) -> ViewModel.HasError, ViewModel.ErrorMessage
```

#### Results Tab

```
PivotItem Header="Results"
  Grid (RowSpacing="0", Padding="0,8,0,0")
    RowDefinitions: [Auto, Auto, *, Auto, Auto]

    Row 0: Toolbar (StackPanel, Horizontal, Spacing="8", Margin="0,0,0,8")
      TextBlock -> ViewModel.StatusMessage
      Button "Columns" -> OnColumnsClick
      Button "CSV" -> OnExportCsvClick
      Button "TSV" -> OnExportTsvClick
      Button "Apply to ADQL" (Visibility=Collapsed, x:Name="ApplyFiltersBtn") -> OnApplyFiltersToAdql
        Icon E71C + "Apply to ADQL"

    Row 1: Sticky header (ScrollViewer x:Name="HeaderScroll")
      HorizontalScrollBarVisibility="Hidden"
      VerticalScrollBarVisibility="Disabled"
      Content: StackPanel x:Name="HeaderPanel" (Spacing="0")

    Row 2: Data rows (ScrollViewer x:Name="DataScroll")
      HorizontalScrollBarVisibility="Auto"
      VerticalScrollBarVisibility="Auto"
      ViewChanged="OnDataScrollViewChanged"
      Content: StackPanel x:Name="ResultsPanel" (Spacing="0")

    Row 3: Pagination (StackPanel, Horizontal, Spacing="12", Padding="0,8")
      TextBlock x:Name="PageStatusText" (CaptionStyle)
      Button "First" (icon EB9E) -> OnFirstPage
      Button "Prev" (icon E76B) -> OnPrevPage
      TextBlock x:Name="PageNumberText" (Margin="4,0")
      Button "Next" (icon E76C) -> OnNextPage
      Button "Last" (icon EB9D) -> OnLastPage
      "Rows per page" label + ComboBox x:Name="RowsPerPageCombo" (Width=80) -> OnRowsPerPageChanged
        ItemsSource: ViewModel.RowsPerPageOptions

    Row 4: Download progress (InfoBar x:Name="DownloadInfoBar")
      IsOpen="False", IsClosable="False", Severity="Informational"
      Content: ProgressBar x:Name="DownloadProgressBar" + TextBlock x:Name="DownloadProgressText"
```

#### ADQL Editor Tab

```
PivotItem Header="ADQL Editor"
  Grid (RowSpacing="8", Padding="0,8,0,0")
    RowDefinitions: [*, Auto]
    Row 0: TextBox (AcceptsReturn, TextWrapping, FontFamily="Consolas", FontSize=13)
      -> ViewModel.AdqlText (TwoWay, PropertyChanged)
    Row 1: Button (AccentStyle) "Execute" with icon E768 -> OnExecuteAdqlClick
      + ProgressRing (20x20) -> ViewModel.IsSearching
```

#### Side Panel

**Recent Searches Card:**
```
Border (Padding="12", CornerRadius="8", CardBackground, CardStroke, BorderThickness="1")
  "Recent Searches" header + "Clear All" button -> OnClearRecentSearches
  ListView (MaxHeight=280, ItemsSource=ViewModel.RecentSearches)
    ItemTemplate:
      Grid (Padding="2,4", ColumnSpacing="4")
        Col 0: Summary (CaptionStyle, ellipsis) + "{ResultCount} results"
        Col 1: Load button (icon E8A5) -> OnLoadRecentSearch (Tag=Binding)
        Col 2: Remove button (icon E74D) -> OnRemoveRecentSearch (Tag=Binding)
```

**Saved Queries Card:**
```
Border (Padding="12", CornerRadius="8", CardBackground, CardStroke, BorderThickness="1")
  "Saved Queries" header
  TextBox x:Name="SaveQueryName" + Save button (icon E74E) -> OnSaveQuery
  ListView (MaxHeight=240, ItemsSource=ViewModel.SavedQueries)
    ItemTemplate:
      Grid (Padding="2,4", ColumnSpacing="4")
        Col 0: Name + ADQL preview (Consolas, MaxLines=2, ellipsis)
        Col 1: Run button (icon E768) -> OnRunSavedQuery
        Col 2: Load button (icon E8DA) -> OnLoadSavedQuery
        Col 3: Delete button (icon E74D) -> OnDeleteSavedQuery
  StackPanel x:Name="QueryRunStatus" (Visibility=Collapsed)
    ProgressRing (16x16) + TextBlock x:Name="QueryRunStatusText"
```

### 5.2 SearchPage.xaml.cs -- Code-Behind

**Namespace:** `CanfarDesktop.Views`
**File:** `Views/SearchPage.xaml.cs`

#### Class Fields

```
SearchViewModel ViewModel
DataLinkService _dataLinkService
ObservationStore _observationStore
DataTrainManager _dataTrainMgr = new()
bool _dataTrainLoaded
bool _dataTrainUIBuilt
bool _suppressTrainEvents
ListView[] _trainLists = []
List<Timer> _filterTimers = []
```

Properties:
- `SpectralUnits -> UnitConverter.SpectralUnits`
- `TimeUnits -> UnitConverter.TimeUnits`
- `PixelScaleUnits -> UnitConverter.PixelScaleUnits`

#### Constructor

```
SearchPage(SearchViewModel viewModel, DataLinkService dataLinkService, ObservationStore observationStore)
```

- Stores dependencies.
- Registers Ctrl+Enter keyboard accelerator -> calls `OnSearchClick`.

#### `void LoadAsync()`

Called externally when page is navigated to:
1. Load recent searches + saved queries from store.
2. If data train not loaded yet: set flag, fire-and-forget `LoadDataTrainInBackground()`.

#### `async Task LoadDataTrainInBackground()`

1. `await ViewModel.LoadDataTrainAsync()` (loads cache + fires network refresh).
2. On dispatcher: `_dataTrainMgr.Load(rows)`, `RebuildAllCheckColumns()`.
3. On error: reset `_dataTrainLoaded = false` (will retry on next visit).

#### Data Train Sync: `SyncDataTrainToViewModel()`

Copies `DataTrainManager.Selected*` (HashSet) to `ViewModel.Selected*` (ObservableCollection) via `CopySet()` -- clear target, add each from source.

#### Search Handlers

##### `OnSearchClick` (async void)
1. `SyncDataTrainToViewModel()`
2. `await ViewModel.SearchCommand.ExecuteAsync(null)`
3. If results: set RowsPerPageCombo, `RenderResultsPage()`, switch to Results tab (index 1).

##### `OnClearClick`
1. `ViewModel.ClearForm()`
2. `_dataTrainMgr.ClearAll()`
3. If UI built: `SyncAllTrainLists()`

##### `OnExecuteAdqlClick` (async void)
1. `await ViewModel.ExecuteAdqlCommand.ExecuteAsync(null)`
2. Same post-processing as OnSearchClick.

#### Data Train UI

##### `RebuildAllCheckColumns()`
- Guards: requires loaded data and not yet built.
- Sets `_dataTrainUIBuilt = true`.
- Creates `_trainLists` array: `[BandList, CollectionList, InstrumentList, FilterList, CalLevelList, DataTypeList, ObsTypeList]`.
- Calls `SyncAllTrainLists()`.

##### `SyncAllTrainLists()`
- Sets `_suppressTrainEvents = true`.
- For each of the 7 ListViews: calls `SyncTrainList(list, available, selected)`.
- Resets `_suppressTrainEvents = false`.

##### `SyncTrainList(ListView, HashSet<string> available, HashSet<string> selected)` (static)
1. Sort available alphabetically -> list.
2. Set `list.ItemsSource = sorted`.
3. For each selected value: find index in sorted list, add to `list.SelectedItems`.

##### `OnTrainSelectionChanged(sender, SelectionChangedEventArgs)`
- Skip if `_suppressTrainEvents` is true.
- Extract column index from `list.Tag` (string "0" to "6" -> int).
- For each added item: call `_dataTrainMgr.Toggle(colIdx, value)` if not already in old selected.
- For each removed item: call `_dataTrainMgr.Toggle(colIdx, value)` if was in old selected.
- `SyncAllTrainLists()` to refresh all downstream lists.

#### Recent Search / Saved Query Handlers

##### `OnLoadRecentSearch`
1. Extract `RecentSearch` from `Button.Tag`.
2. `ViewModel.LoadFromRecentSearch(search)`.
3. If data train UI built: restore DataTrainManager selections from ViewModel, call `Refresh()`, `SyncAllTrainLists()`.
4. Switch to Search Form tab (index 0).

##### `OnRemoveRecentSearch` -- delegates to `ViewModel.RemoveRecentSearch`.
##### `OnClearRecentSearches` -- delegates to `ViewModel.ClearAllRecentSearches`.

##### `OnSaveQuery`
- Get name from TextBox (default: `"Query {now:yyyy-MM-dd HH:mm}"`).
- `ViewModel.SaveCurrentQuery(name)`, clear TextBox.

##### `OnLoadSavedQuery`
- `ViewModel.LoadSavedQuery(query)`, switch to ADQL Editor tab (index 2).

##### `OnRunSavedQuery` (async void)
1. Show progress panel.
2. Set AdqlText, execute.
3. Hide progress, render results if successful, switch to Results tab.

##### `OnDeleteSavedQuery` -- delegates to `ViewModel.DeleteSavedQuery`.

#### Results Table Rendering

##### `RenderResultsPage(bool rebuildHeader = true)`

1. If `rebuildHeader`:
   - Dispose filter timers.
   - Clear `HeaderPanel`.
   - If results non-empty: get visible column keys, build header row + filter row.

2. Clear `ResultsPanel`.

3. If no results: show "No results found." TextBlock, update pagination, return.

4. For each row in `ViewModel.GetCurrentPageRows()`:
   - Build row border via `BuildRow(keys, isHeader: false, row, rowIndex)`.
   - **Tapped handler**: Show row detail (but NOT if tapped element has Tag="action" or an ancestor with Tag="action").
   - **PointerEntered**: set background to hover color.
   - **PointerExited**: restore original background.
   - **Alternating rows**: odd rows get `CardBackgroundFillColorDefaultBrush`.

5. `UpdatePaginationUI()`.

##### Sticky Header Sync: `OnDataScrollViewChanged`

```csharp
HeaderScroll.ChangeView(DataScroll.HorizontalOffset, null, null, disableAnimation: true);
```

This synchronizes horizontal scroll between the header and data areas.

##### `Border BuildFilterRow(string[] columnKeys)`

For each column key:
- If `download` or `preview`: empty Border with matching width (no filter).
- Otherwise: TextBox with:
  - Width matching column
  - Height=24, FontSize=11, Padding=(4,2,4,2)
  - PlaceholderText="Filter..."
  - Pre-filled with existing filter value.
  - TextChanged handler with **300ms debounce timer**:
    1. On each keystroke: cancel previous timer, start new 300ms timer.
    2. On timer fire (dispatcher): set filter, update pagination, re-render rows (NOT header), update "Apply to ADQL" button visibility.

Background: `CardBackgroundFillColorSecondaryBrush`, Padding=(0,2,0,2).

##### `Border BuildRow(string[] columnKeys, bool isHeader, SearchResultRow? row, int rowIndex)`

For each column key:

**Download button cell** (data row, non-empty publisherID):
- Button with icon `\uE896`, Padding=2, transparent bg, Width=column width, Tag="action"
- Click -> `DownloadFileAsync(publisherID, row)`

**Preview button cell** (data row, non-empty publisherID):
- Button with "Preview" text, Padding=2, transparent bg, Width=column width, Tag="action"
- Has Flyout that loads on first open:
  1. Show ProgressRing.
  2. Resolve DataLink for publisherID.
  3. Get first thumbnail URL or first preview URL.
  4. Load image via `LoadImageFromUrlAsync`.
  5. Display Image (MaxWidth=300, MaxHeight=300, Uniform stretch) or "No preview available".

**Sortable header cell** (header row, not download/preview):
- TextBlock with label + sort indicator (`\u25B2` up or `\u25BC` down if current sort column).
- Width=column width, FontSize=12, SemiBold, CharacterEllipsis.
- Tapped -> `ViewModel.SortBy(key)`, re-render full results.
- PointerEntered -> Opacity=0.6, PointerExited -> Opacity=1.0.

**Normal cell** (data row or header for download/preview):
- TextBlock with formatted value (`ViewModel.FormatCell(key, rawValue)`) or label.
- Width=column width, FontSize=12, Normal/SemiBold weight.

Row border: MinHeight=28. Header rows: `CardBackgroundFillColorSecondaryBrush`. Odd data rows: `CardBackgroundFillColorDefaultBrush`.

StackPanel inside: Orientation=Horizontal, Padding=(4,5,4,5), Spacing=2.

##### Column Selector Dialog: `OnColumnsClick` (async)

Opens `ColumnSelectorDialog` with current `ResultColumns` list. Dialog shows checkboxes in 3-column grid layout. On Primary result: applies selections back to ViewModel, re-renders.

The dialog:
- Calculates `rowsPerCol = ceil(columns.Count / 3)`
- Creates checkboxes for each column, laid out in 3 Grid columns
- CheckBox: Content = Label, IsChecked = Visible, FontSize=13

##### Pagination Handlers

- `OnFirstPage` / `OnPrevPage` / `OnNextPage` / `OnLastPage`: delegate to ViewModel, re-render rows only (no header rebuild).
- `OnRowsPerPageChanged`: set ViewModel.RowsPerPage, reset to page 1, re-render.
- `UpdatePaginationUI()`: calls `ViewModel.UpdatePagination()`, updates PageStatusText and PageNumberText (`"Page {current} / {total}"`).

##### `OnApplyFiltersToAdql`

Calls `ViewModel.BuildFilteredAdql()`. If non-null, sets `ViewModel.AdqlText` and switches to ADQL Editor tab (index 2).

##### `UpdateApplyFiltersButton()`

Shows/hides "Apply to ADQL" button based on `ViewModel.HasActiveFilters`.

#### Row Detail Modal: `ShowRowDetail(SearchResultRow row)`

1. Get publisherID from row.
2. Build StackPanel (Spacing=12):
   a. **Image section**: Horizontal StackPanel inside ScrollViewer (MaxHeight=200). Shows ProgressRing while loading.
   b. **Action buttons**: Download button (icon `\uE896`, "Download") -- closes dialog first, then downloads.
   c. **Metadata fields**: For each column in `ResultColumns`, create label+value row via `UIFactory.CreateMetadataRow(label, formattedValue, 170)`. Skips empty values.

3. Wrap in ContentDialog:
   - Title: `"Observation -- {targetName}"` or `"Observation Detail"`.
   - Content: ScrollViewer (MaxHeight=550).
   - CloseButtonText: "Close".
   - MinWidth: 650.

4. Fire-and-forget `LoadDetailImagesAsync` to load preview images.

##### `LoadDetailImagesAsync(publisherID, imagePanel, spinner, section)`

1. Resolve DataLink.
2. Get up to 3 image URLs (previews first, then thumbnails, distinct).
3. Hide spinner.
4. If no images: collapse section.
5. For each URL: load via `LoadImageFromUrlAsync`, add Image (MaxHeight=180, MaxWidth=250, Uniform stretch).

##### `LoadImageFromUrlAsync(string url) -> Task<BitmapImage?>`

1. Call `_dataLinkService.DownloadImageBytesAsync(url)`.
2. If null/empty: return null.
3. Create `BitmapImage`, set source from `MemoryStream`.

#### Download Flow: `DownloadFileAsync(string publisherID, SearchResultRow? sourceRow)`

**Step 1 -- Resolve DataLink:**
```
dataLink = await _dataLinkService.GetLinksAsync(publisherID)
url = dataLink.DirectFileUrl ?? _dataLinkService.GetDownloadUrl(publisherID)
```

**Step 2 -- File selection (if multiple):**
- If `dataLink.DirectFiles.Count > 1`: show `ShowFileSelectionDialogAsync` dialog.
  - ListView with single selection, MaxHeight=300.
  - Each item: StackPanel with Filename (bold), Description (caption), ContentType (caption).
  - First item pre-selected.
  - On Primary: return selected `DataLinkFile`. On Cancel: return null (abort download).
- If exactly 1 file: use its filename.
- If 0 files: no filename hint.

**Step 3 -- File picker:**
- Get window handle via `WindowHelper.ActiveWindows[0]`.
- Open `FileSavePicker` with:
  - SuggestedFileName: from DataLink, or extracted from publisherID.
  - Extension added if missing: `.fits`.
  - FileTypeChoices: "FITS Image" (.fits), "All Files" (.).
- If cancelled: return.

**Filename extraction from publisherID:**
```
"ivo://cadc.nrc.ca/CFHT?1100689/1100689o" -> "1100689o" (after last /)
"ivo://cadc.nrc.ca/CFHT?1100689" -> "1100689" (after last ?)
fallback: "observation"
```

**Step 4 -- Download with progress:**
1. Show InfoBar (`DownloadInfoBar.IsOpen = true`), set indeterminate progress.
2. Download to temp file (`file.Path + ".tmp"`).
3. If Content-Length available: switch to determinate progress.
4. Read in 81920-byte chunks, update:
   - `DownloadProgressBar.Value = downloaded`
   - `DownloadProgressText = "{downloaded} / {total} ({pct}%)"` or just `"{downloaded}"` if no Content-Length.
5. Flush, rename temp -> final path.
6. Show success InfoBar for 3 seconds, then auto-close.
7. On error: close InfoBar, delete temp file.

**Byte formatting:**
```
< 1024: "{bytes} B"
< 1MB: "{KB:F1} KB"
< 1GB: "{MB:F1} MB"
>= 1GB: "{GB:F2} GB"
```

**Step 5 -- Track in ObservationStore:**
- If sourceRow is provided: create `DownloadedObservation.FromSearchResult(row, filePath, dataLink, getHeader)`.
- Set file size.
- Save to observation store.

#### Export Handlers

##### `OnExportCsvClick` / `OnExportTsvClick` (async void)

Calls `ExportFileAsync(content, suggestedName, extension, formatLabel)`:
1. Get window handle.
2. Open `FileSavePicker` with suggested name and type.
3. Write content to file.

#### Helper Classes (in same file)

##### `WindowHelper` (internal static)
- `ActiveWindows: List<Window>` = `[]`
- `TrackWindow(Window)`: adds to list, registers Closed handler to remove.

##### `FrameworkElementExtensions` (internal static)
- `FindParentWithTag(FrameworkElement, string tag) -> FrameworkElement?`: walks up visual tree looking for element with matching `Tag`.

---

## API Endpoints Reference

| Endpoint | URL Template |
|---|---|
| TAP sync | `https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/argus/sync` |
| Target resolver | `https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/cadc-target-resolver/find` |
| DataLink | `https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops/datalink?id={pubID}&request=downloads-only` |
| Download package | `https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops/pkg?ID={pubID}` |

---

## Key Behavioral Notes for Port

1. **CSV parsing** uses a character-by-character state machine for proper RFC 4180 handling (quoted fields, escaped quotes).

2. **Data train cascade** is the same algorithm in both `DataTrainManager` (pure logic) and `SearchViewModel` (with ObservableCollections). The port should use a single implementation.

3. **Debounce** is used in two places: target resolver (500ms) and per-column filters (300ms).

4. **DataLink caching** is in-memory with ConcurrentDictionary. Failures are NOT cached to allow retry. Successes (including empty results) ARE cached.

5. **Image downloads** are concurrency-limited to 3 via semaphore, with single retry on transient errors.

6. **Sticky header** works by having two separate ScrollViewers -- the header one has HorizontalScrollBarVisibility="Hidden" and its offset is synced on every ViewChanged event from the data ScrollViewer.

7. **Sorting** is smart: numeric if both values parse, otherwise string. Empties always sort last.

8. **The download flow** writes to a temp file first, then renames, to avoid corrupted files on cancellation/failure.

9. **Row detail dialog** hides itself before starting a download to avoid the WinUI "only one ContentDialog" limitation.

10. **FilterToAdqlConverter** converts client-side column filters into ADQL WHERE clauses, allowing the user to refine a search server-side.
