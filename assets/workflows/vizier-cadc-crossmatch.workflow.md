# Catalogue cross-match sample builder (VizieR × CADC)
> Define a sample from a VizieR catalogue, find which members have CADC archival data, and build a prioritized target list.
Tags: catalogues, VizieR, Gaia, sample-selection
Time: ~2 h

## Steps

- [ ] **Define the parent sample** — Cone-search a VizieR catalogue (e.g. Gaia DR3, I/355/gaiadr3)
      around your field with your photometric/astrometric cuts.
      Tool: vizier_cone_search
      Note: Keep the cuts in writing — they are the sample definition your paper will quote.
- [ ] **Check CADC holdings per candidate** — For each sample member (or the top N), run a small
      cone search of the CADC archive at its position: what imaging/spectroscopy exists?
      Tool: search_observations
      View: search
      Note: This is a natural step to delegate to the agent — a loop of per-position searches.
- [ ] **Build the priority list** — Rank candidates by data richness: has imaging + spectroscopy >
      imaging only > nothing (needs new observations).
      View: research
- [ ] **Save the defining queries** — The VizieR cuts and the archive search both become saved
      queries for reproducibility.
      Tool: save_query
- [ ] **Bulk-download the top priority data** — Pull the best archival observations for the
      highest-ranked members into the research archive.
      Tool: download_observations_bulk
      View: research
- [ ] **Annotate every member** — One note per object: rank, available data, and what analysis it
      feeds.
      Tool: bulk_update_observation_notes
      View: research
- [ ] **Export the bundle for the team** — Research bundle (data + notes) so collaborators start
      from the same curated sample.
      Tool: export_research_bundle
      View: research
