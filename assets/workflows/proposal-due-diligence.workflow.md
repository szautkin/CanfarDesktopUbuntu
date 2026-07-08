# Observing-proposal due diligence (CFHT / Gemini call)
> Before requesting new telescope time: prove what the archive already holds for each target, and document the gap your proposal fills.
Tags: proposal, CFHT, Gemini, archival
Time: ~2 h

## Steps

- [ ] **List the proposal targets** — Write out every target with coordinates; resolve each so
      archive searches are exact.
      Tool: resolve_target
      View: search
- [ ] **Archive-check each target** — For every target: search the CADC archive (CFHT, Gemini,
      HST holdings) for existing data in your required instrument/filter/wavelength.
      Tool: search_observations
      View: search
      Note: Time Allocation Committees ask "why can't this be done with archival data?" — this
      step is your answer.
- [ ] **Record the depth of what exists** — Per target: deepest existing exposure per band,
      epoch coverage, and calibration state.
      Tool: get_observation_caom2
      View: search
- [ ] **Identify the gaps** — Which targets genuinely lack the required filter/depth/epoch?
      That gap list is the core of the technical justification.
      View: research
- [ ] **Pull feasibility examples** — Download one representative archival observation per
      target class to demonstrate feasibility (source detectable, field uncrowded).
      Tool: download_observation
      View: research
- [ ] **Make the feasibility figures** — Quick-look figures from the archival data in a notebook
      (target visible, S/N estimate scales to your requested time).
      Tool: create_analysis_notebook
      View: notebook
- [ ] **Write the gap-analysis summary** — Per-target notes rolled into a summary the co-Is can
      lift into the proposal text.
      Tool: update_observation_note, export_research_bundle
      View: research
