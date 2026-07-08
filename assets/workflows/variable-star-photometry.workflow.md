# Variable-star time-series photometry (CFHT / NEOSSat)
> Find multi-epoch imaging of a field, build light curves, and search for variability.
Tags: photometry, variability, time-series, CFHT, NEOSSat
Time: ~3 h

## Steps

- [ ] **Resolve the field** — Cluster or field centre coordinates; note the field of view you
      need to cover.
      Tool: resolve_target
      View: search
- [ ] **Query multi-epoch imaging in one filter** — Search CFHT MegaCam (or NEOSSat for bright
      targets) at the field position; a single consistent filter keeps the photometry differential.
      Tool: search_observations
      View: search
- [ ] **Assess the cadence** — Sort epochs by date: how many epochs, over what baseline, with
      what gaps? Decide whether the sampling supports your expected periods.
      View: search
      Note: Aliasing: nightly cadence hides periods near 1 day and its harmonics.
- [ ] **Save the query** — The epoch selection is a result in itself; keep it reproducible.
      Tool: save_query
- [ ] **Bulk-download the series** — Pull every usable epoch into the research archive in one go.
      Tool: download_observations_bulk
      View: research
- [ ] **Create the photometry notebook** — Seed an aperture-photometry notebook for the first
      downloaded observation (template: photometry), then generalize it over all epochs.
      Tool: create_analysis_notebook
      View: notebook
- [ ] **Measure per-epoch photometry** — Aperture photometry of the target and 5–10 comparison
      stars in every epoch; build differential magnitudes against the comparison ensemble.
      Tool: run_all_cells
      View: notebook
- [ ] **Search for variability** — Plot light curves; compute scatter vs magnitude to find
      outliers; run a Lomb–Scargle periodogram on candidates.
      View: notebook
- [ ] **Publish the products** — Save light-curve tables and the notebook to VOSpace so the
      analysis travels with the data.
      Tool: upload_file_to_vospace, create_vospace_folder
      View: storage
- [ ] **Log candidates** — One note per candidate variable: period, amplitude, classification
      guess, follow-up needed.
      Tool: update_observation_note, bulk_update_observation_notes
      View: research
