# Archival imaging reconnaissance (CFHT MegaCam)
> What imaging already exists for my object? Find, vet, and download the best CFHT MegaCam data.
Tags: imaging, CFHT, MegaCam, archival
Time: ~1 h

## Steps

- [ ] **Resolve the target** — Confirm coordinates and common aliases before searching, so the
      cone search is centred correctly and you recognize cross-listed observations.
      Tool: resolve_target
      View: search
- [ ] **Cone-search CADC for MegaCam imaging** — Search collection CFHT, instrument MegaCam,
      radius 10–15 arcmin around the resolved position.
      Tool: search_observations
      View: search
- [ ] **Filter to science-ready data** — Keep calibration level 2+ (calibrated) images only;
      group by filter and sort by integration time to spot the deepest exposures.
      View: search
      Note: Raw (level 0/1) frames need detrending you probably don't want to redo.
- [ ] **Inspect previews** — Open previews of the deepest candidates per filter; discard bad
      seeing, satellite trails, or fields where the target sits on a chip edge.
      Tool: get_preview_image
      View: search
- [ ] **Save the query** — Store the exact search so the selection is reproducible in your paper
      and re-runnable when new data appear.
      Tool: save_query
- [ ] **Download the best exposure per filter** — Pull the selected observations into the local
      research archive.
      Tool: download_observation, download_observations_bulk
      View: research
- [ ] **Verify WCS in the FITS viewer** — Open each image and jump to a catalogue star of known
      position; confirm the WCS lands on it.
      Tool: open_fits_file, fits_goto_coordinate, get_fits_wcs
      View: fitsViewer
- [ ] **Bookmark key coordinates** — Save the target and any comparison/reference positions for
      quick return later.
      Tool: save_fits_bookmark
      View: fitsViewer
- [ ] **Record what you found** — Note per observation: filter, depth, seeing estimate, and
      whether it is usable for your science case.
      Tool: update_observation_note
      View: research
