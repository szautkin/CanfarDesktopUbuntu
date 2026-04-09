# Search Module — Windows Parity Implementation Plan

## Gap Analysis: Current Ubuntu vs Windows v1.1.0

### Models (search_result.rs)
| Feature | Windows | Ubuntu | Gap |
|---------|:---:|:---:|-----|
| SearchFormState fields | 37 | 30 | Missing: PixelScaleUnit, SpatialCutout, DateStart, DateEnd, IntegrationTimeUnit, TimeSpanUnit, SpectralCoverageUnit, SpectralSamplingUnit, BandpassWidthUnit, RestFrameEnergyUnit, SpectralCutout, Bands/Collections/Instruments/Filters/CalibrationLevels/DataProductTypes/ObservationTypes as separate fields |
| SearchFormState.summary() | Yes | Yes | OK |
| DataLinkResult | Thumbnails + Previews + DirectFiles + DownloadUrl | files list + download_url | Need to split into Thumbnails/Previews/DirectFiles by semantics |
| DataTrainRow | Dedicated struct | No struct | Missing — need for cascade filtering |
| ResultColumnInfo.Width | Per-column px widths | No widths | Missing width table |
| ResultColumnInfo.Header | Original CSV header as lookup key | key used as lookup | Need Header field |
| RecentSearch.FormState | Full FormState saved | Full FormState saved | OK |

### Services
| Feature | Windows | Ubuntu | Gap |
|---------|:---:|:---:|-----|
| TAPService.execute_query | POST with CSV parsing | POST with CSV parsing | OK |
| TAPService.resolve_target | GET with ASCII parsing | GET with ASCII parsing | OK |
| TAPService.fetch_data_train | Returns Vec<DataTrainRow> | Returns DataTrain (distinct sets) | Need raw rows for cascade filtering |
| DataLinkService.resolve | VOTable regex parsing, cache, retry | VOTable roxmltree parsing, cache | OK (roxmltree is better than regex) |
| DataLinkService.download_image | Semaphore(3), 2 retries, 300ms delay | Semaphore(3), no retry | Need retry logic |
| SearchStoreService | load/save recent(20), load/save/delete queries | load/save recent(20), load/save/delete queries | OK |

### Helpers
| Feature | Windows | Ubuntu | Gap |
|---------|:---:|:---:|-----|
| ADQLBuilder SELECT | 41 columns | 24 columns | Missing 17 columns |
| ADQLBuilder WHERE — observation | wildcards, LIKE | wildcards, LIKE | OK |
| ADQLBuilder WHERE — spatial | INTERSECTS + pixel scale | INTERSECTS + pixel scale | OK |
| ADQLBuilder WHERE — temporal | INTERSECTS INTERVAL, MJD, date presets, date expansion, integration time with inline unit suffix | Simple range, date presets | Missing: INTERSECTS INTERVAL, date expansion, inline unit suffix extraction |
| ADQLBuilder WHERE — spectral | Overlap semantics, unit conversion (freq/energy/wavelength), inline suffix | Simple range | Missing: overlap semantics, frequency/energy conversion |
| RangeParser | Dedicated parser (A..B, >=, <=, >, <, =) | Inline in form state (wavelength_min/max separate fields) | Missing: need unified range parser |
| UnitConverter | Wavelength/freq/energy/time/angle conversions | Wavelength only (nm/um/A/mm/cm/m) | Missing: frequency, energy, time, angle conversions |
| CellFormatter | 11 format types, MJD(40587 epoch), adaptive int time, column widths | 7 format types, MJD(J2000 epoch) | Wrong MJD epoch! Must use 40587.0 not J2000. Missing FormatBoolean, FormatWavelength, FormatScientific, FormatTimestamp |
| DataTrainManager | Cascade filtering with 7 levels, toggle, clear, Available*/Selected* sets | No manager | Missing entirely — critical for data train UI |
| FilterToAdqlConverter | In-memory filter to ADQL append | Not implemented | Missing |
| ResultFilter | In-memory substring filter across columns | Not implemented | Missing |
| ResultSorter | Smart sort (numeric vs string, empty last) | Not implemented | Missing |

### UI (search_page.rs)
| Feature | Windows | Ubuntu | Gap |
|---------|:---:|:---:|-----|
| Layout | Main(tabs) + Sidebar(260px) | Main(tabs) + Sidebar(260px) | OK |
| Form tab | 4 columns side-by-side | 4 columns side-by-side | OK |
| All form fields | ~25 fields with unit combos | ~20 fields with unit combos | Missing: pixel scale unit combo, spectral sampling unit, bandpass width unit, rest frame energy unit |
| Data train | 7 cascade-filtered multi-select lists | 7 lists loading from TAP | Missing: cascade filtering, selection→ADQL |
| Resolve status | ProgressRing + text | Label only | Missing: spinner during resolve |
| Results tab | Sticky header, per-column filter, sort by click, alternating rows, row detail modal | Simple header + rows | Missing: per-column filter, sort, row detail, sticky header |
| Pagination | First/Prev/Next/Last + rows-per-page combo | First/Prev/Next/Last | Missing: rows-per-page selector |
| Column picker | Dialog with checkboxes | Not implemented | Missing |
| CSV/TSV export | Two buttons | Not implemented | Missing |
| Download flow | DataLink→multi-file dialog→progress→ObservationStore | Not implemented | Missing |
| Row detail modal | Full metadata + preview images | Not implemented | Missing |
| ADQL tab | Monospace editor + Execute | Monospace editor + Execute | OK |
| Sidebar recent | Summary + count + Load + Remove buttons | Summary + count + Load | Missing: Remove button per item |
| Sidebar saved | Name + ADQL preview + Run + Load + Delete | Name + ADQL preview + Run + Load + Delete | OK |
| Keyboard | Ctrl+Enter to search | Not implemented | Missing |

## Implementation Steps (Priority Order)

### Step 1: Fix CellFormatter MJD epoch (CRITICAL BUG)
Current code uses J2000 epoch. Windows uses Unix epoch (MJD 40587.0).
File: `src/models/search_result.rs` function `mjd_to_date`

### Step 2: Add missing SELECT columns to ADQL builder
File: `src/helpers/adql_builder.rs` — add 17 missing columns

### Step 3: Add RangeParser + UnitConverter helpers
New file: `src/helpers/range_parser.rs`
New file: `src/helpers/unit_converter.rs`

### Step 4: Upgrade ADQL temporal clauses
Use INTERSECTS INTERVAL, date expansion, inline unit suffix extraction

### Step 5: Upgrade ADQL spectral clauses
Overlap semantics, frequency/energy conversion

### Step 6: Add DataTrainManager with cascade filtering
New file: `src/helpers/data_train_manager.rs`

### Step 7: Wire data train cascade into search_page.rs UI
Connect ListBox selections → DataTrainManager → refresh available lists

### Step 8: Add ResultFilter + ResultSorter
New file: `src/helpers/result_filter.rs`

### Step 9: Upgrade results tab — sortable columns, per-column filter, rows-per-page
File: `src/ui/search_page.rs`

### Step 10: Add column picker dialog
### Step 11: Add CSV/TSV export
### Step 12: Add Ctrl+Enter keyboard shortcut
### Step 13: Add row detail modal with preview images
### Step 14: Add download flow (DataLink → file picker → progress)
