# Archival stellar spectroscopy (DAO Plaskett / CFHT ESPaDOnS)
> Gather archival spectra of a star across epochs and measure radial velocities and line strengths.
Tags: spectroscopy, DAO, ESPaDOnS, radial-velocity
Time: ~2 h

## Steps

- [ ] **Resolve the star** — Confirm coordinates and aliases (HD / BD / Gaia designations) so no
      archival epoch is missed under another name.
      Tool: resolve_target
      View: search
- [ ] **Search DAO spectra** — Collection DAO (1.8 m Plaskett): what epochs and dispersions exist?
      Tool: search_observations, save_query
      View: search
- [ ] **Search CFHT ESPaDOnS spectra** — A second saved query for high-resolution echelle epochs
      (R ~ 68 000, often with polarimetry).
      Tool: search_observations, save_query
      View: search
- [ ] **Choose the epoch set** — Balance resolution, signal-to-noise, and time baseline for your
      question (binarity needs baseline; line profiles need resolution).
      View: search
- [ ] **Download the 1D products** — Calibrated, extracted spectra where available.
      Tool: download_observations_bulk
      View: research
- [ ] **Normalize in a notebook** — Load each spectrum, fit and divide the continuum, and put all
      epochs on a common wavelength grid.
      Tool: create_analysis_notebook
      View: notebook
- [ ] **Measure RVs and equivalent widths** — Cross-correlate against a template (or fit line
      cores) per epoch; measure EWs of your diagnostic lines.
      View: notebook
- [ ] **Compare across epochs** — RV variation → binarity or pulsation; EW/profile variation →
      activity or spots. Plot against time and phase-fold trial periods.
      View: notebook
- [ ] **Record the measurements** — Measurement table to VOSpace; per-observation notes with the
      derived RVs and any anomalies.
      Tool: upload_file_to_vospace, update_observation_note
      View: research
