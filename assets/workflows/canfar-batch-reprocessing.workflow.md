# Batch reprocessing on CANFAR (headless jobs)
> Run the same processing over many files with headless replicas on the CANFAR science platform.
Tags: CANFAR, batch, headless, reprocessing
Time: ~half a day (mostly waiting)

## Steps

- [ ] **Stage the inputs in VOSpace** — Create a project folder and upload (or collect) the input
      file list the jobs will read.
      Tool: create_vospace_folder, upload_file_to_vospace, list_vospace_path
      View: storage
- [ ] **Check your quota** — Outputs need room; confirm usage before generating N × products.
      Tool: get_storage_quota
      View: storage
- [ ] **Pick the container image** — Find an image that already carries your software stack
      (e.g. astropy + your reduction package) instead of building one.
      Tool: find_images_with_packages
      View: portal
- [ ] **Dry-run one job** — Launch a single replica on one input with a short command; this
      catches path and environment mistakes cheaply.
      Tool: launch_headless_job
      View: portal
- [ ] **Read the logs and events** — Confirm the dry run produced the expected output and clean
      logs before scaling.
      Tool: get_headless_job_logs, get_headless_job_events
      View: portal
- [ ] **Scale to the full set** — Launch the production run with replicas sized to the input list
      (max 20 per launch; chunk the list if larger).
      Tool: launch_headless_job
      View: portal
- [ ] **Monitor to completion** — Watch job states; investigate any replica that fails rather
      than silently accepting partial output.
      Tool: list_headless_jobs, get_headless_job_logs
      View: portal
- [ ] **Verify the outputs in VOSpace** — Count and spot-check products against the input list —
      every input should have its output.
      Tool: list_vospace_path, read_vospace_file
      View: storage
- [ ] **Record the run** — Note the image tag, command line, replica count, and date — the
      reproducibility record for the paper's data-processing section.
      View: research
