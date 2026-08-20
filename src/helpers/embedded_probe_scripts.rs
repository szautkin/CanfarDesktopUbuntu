//! The contract between this app and the two probe scripts.
//!
//! Port of `Services/ImageDiscovery/EmbeddedProbeScripts.cs`. The script bodies
//! are compiled into the binary via [`include_str!`]; their VOSpace upload
//! filenames are content-hashed (`probe-<12hex>.sh`) so editing a script
//! automatically busts the previously-uploaded copy.
//!
//! Everything both sides have to agree on lives here: the bodies, the upload
//! names, the home subdirectory, and — since the scripts write a manifest the
//! app then reads back — how an image id becomes a filename. Splitting that
//! last rule across the two languages is how a reader looks in a place the
//! writer never wrote to, so [`sanitize_image_id`] is checked against the
//! scripts' own `tr` set rather than trusted to match it.

use sha2::{Digest, Sha256};

/// Home-relative subdirectory the scripts are uploaded to (`~/.verbinal`).
///
/// Mirrors `EmbeddedProbeScripts.HomeSubdirectory` and the `.verbinal` path the
/// scripts themselves hard-code.
pub const HOME_SUBDIR: &str = ".verbinal";

/// In-container probe script (runs inside the target image; emits a manifest).
const PROBE_SCRIPT: &str = include_str!("../resources/imagedisc/probe.sh");

/// Out-of-band inspector script (syft-based static scan of a target image from a
/// known-good headless container).
const INSPECTOR_SCRIPT: &str = include_str!("../resources/imagedisc/inspector.sh");

/// Body of the in-container probe script.
pub fn probe_script() -> &'static str {
    PROBE_SCRIPT
}

/// Body of the out-of-band inspector script.
pub fn inspector_script() -> &'static str {
    INSPECTOR_SCRIPT
}

/// Lowercase hex SHA-256 digest of `text`.
fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Content-hashed upload filename for the probe script (`probe-<first-12-hex>.sh`).
///
/// Mirrors `EmbeddedProbeScripts.ProbeUploadFileName`.
pub fn probe_script_name() -> String {
    format!("probe-{}.sh", &sha256_hex(PROBE_SCRIPT)[..12])
}

/// Content-hashed upload filename for the inspector script
/// (`inspector-<first-12-hex>.sh`). Mirrors `InspectorUploadFileName`.
pub fn inspector_script_name() -> String {
    format!("inspector-{}.sh", &sha256_hex(INSPECTOR_SCRIPT)[..12])
}

/// Turn an image id into a filename component, exactly as the scripts do.
///
/// The scripts are the WRITER — they publish the manifest — so their rule is
/// the rule, and this follows it rather than the other way round.
/// [`tests::the_sanitiser_matches_the_one_the_scripts_use`] reads the character
/// set out of the scripts to make sure.
pub fn sanitize_image_id(image_id: &str) -> String {
    image_id
        .chars()
        .map(|c| if SANITIZED.contains(&c) { '_' } else { c })
        .collect()
}

/// Characters the scripts collapse to `_`. Mirrors their `tr` set.
const SANITIZED: &[char] = &['/', ':', '?', '*', '<', '>', '|', '"', '\\'];

/// Where a manifest for `image_id` lands, relative to the user's home.
///
/// One spelling of this path, used by the scripts' Rust-side reader and by
/// nothing else — the scripts hard-code the same shape, and the guard above
/// ties the two together at the only part that varies.
pub fn manifest_path(image_id: &str) -> String {
    format!(
        "{HOME_SUBDIR}/manifests/{}.json",
        sanitize_image_id(image_id)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_shebanged_and_carry_their_env_contract() {
        assert!(probe_script().starts_with("#!/usr/bin/env bash"));
        assert!(inspector_script().starts_with("#!/usr/bin/env bash"));
        // Each script keys off a launcher-supplied env var.
        assert!(probe_script().contains("IMAGE_ID"));
        assert!(inspector_script().contains("TARGET_IMAGE"));
    }

    /// Every path that publishes a manifest must also print it.
    ///
    /// This port recovers the manifest from the job's STDOUT — the module doc
    /// on `ImageDiscoveryCoordinator` says so — and then deletes the job. Both
    /// scripts were writing to `$OUT` and echoing only `ok: $OUT`, so every
    /// inspection reported "job produced no manifest JSON in its logs", and
    /// every `probeNotes` explanation of WHY a probe gave up was written to a
    /// file inside a container nobody would ever open.
    #[test]
    fn every_published_manifest_also_reaches_stdout() {
        for (name, script) in [
            ("probe.sh", probe_script()),
            ("inspector.sh", inspector_script()),
        ] {
            let published = script.matches(r#"mv "$TMP" "$OUT""#).count();
            let printed = script.matches(r#"cat "$OUT""#).count();
            assert!(published > 0, "{name} no longer publishes a manifest");
            assert_eq!(
                printed, published,
                "{name} publishes {published} manifests but prints {printed}; \
                 the ones it does not print are invisible to the app"
            );
        }
    }

    /// Status chatter belongs on stderr, so stdout is the manifest and nothing
    /// else. `extract_manifest_json` tolerates noise, but a reader of the logs
    /// should not have to.
    #[test]
    fn status_lines_go_to_stderr() {
        for (name, script) in [
            ("probe.sh", probe_script()),
            ("inspector.sh", inspector_script()),
        ] {
            for line in script.lines() {
                let line = line.trim();
                // Only bare echoes. A line redirecting into a staging file
                // (`echo "$sh" >> "$STAGE/shells.txt"`) is data collection, not
                // chatter, and its `>` says so.
                if !line.starts_with("echo \"") || line.contains('>') {
                    continue;
                }
                panic!("{name} writes a status line to stdout: {line}");
            }
        }
    }

    /// Constructs that exist in GNU coreutils and not in BusyBox.
    ///
    /// Both scripts run inside a container the app did not build: the inspector
    /// image is Alpine (BusyBox), and the in-target probe runs inside whatever
    /// the user asks about. Every utility they reach for has to be the POSIX
    /// one.
    ///
    /// `mktemp --suffix=.py` is the entry that put this list here. BusyBox
    /// printed its usage, the assignment came back empty, and the next line —
    /// `cat > "$TRANSFORMER"` — died with "No such file or directory" naming a
    /// line number and nothing else. Every inspection on Alpine failed there.
    const GNU_ONLY: &[(&str, &str)] = &[
        (
            "mktemp --",
            "BusyBox mktemp takes only [-dqtup] and a TEMPLATE",
        ),
        ("grep -P", "BusyBox grep has no PCRE mode"),
        ("find -printf", "GNU find extension"),
        ("-printf ", "GNU find extension"),
        ("sort -V", "GNU version sort"),
        ("--suffix=", "GNU long option"),
        ("cp --", "GNU long option"),
        ("ls --", "GNU long option"),
        ("date --", "GNU long option"),
        ("head --", "GNU long option"),
        ("tail --", "GNU long option"),
        ("wc --", "GNU long option"),
        ("sed --", "GNU long option"),
        ("tr --", "GNU long option"),
    ];

    #[test]
    fn the_scripts_use_no_gnu_only_constructs() {
        for (name, script) in [
            ("probe.sh", probe_script()),
            ("inspector.sh", inspector_script()),
        ] {
            for (needle, why) in GNU_ONLY {
                // Comments explaining the rule are not violations of it.
                let offending = script
                    .lines()
                    .filter(|l| !l.trim_start().starts_with('#'))
                    .find(|l| l.contains(needle));
                assert!(
                    offending.is_none(),
                    "{name} uses {needle:?} ({why}): {}",
                    offending.unwrap().trim()
                );
            }
        }
    }

    /// Both probe paths must name an OS the same way.
    ///
    /// The in-container probe reads os-release `ID` / `VERSION_ID`; the
    /// inspector reads syft's `distro`, which mirrors os-release field for
    /// field. Reading `name` / `version` there instead described the same image
    /// as "alpine linux" / "3.20.3" from one path and "alpine" / "3.20" from
    /// the other — so the discovery facets listed both and a filter on either
    /// matched half the images.
    #[test]
    fn both_paths_name_an_os_the_same_way() {
        assert!(probe_script().contains(r#"data.get("ID""#));
        assert!(probe_script().contains(r#"data.get("VERSION_ID""#));
        assert!(inspector_script().contains(r#"distro.get("id")"#));
        assert!(inspector_script().contains(r#"distro.get("versionID")"#));
    }

    /// Run both scripts under a BusyBox-only PATH and require a real manifest.
    ///
    ///     cargo test --quiet under_busybox -- --ignored --nocapture
    ///
    /// The deny-list above catches the GNU-isms someone thought to write down.
    /// This catches the rest, by being the environment the inspector image
    /// actually is: Alpine, where every coreutil is a BusyBox applet.
    ///
    /// Ignored rather than skipped-when-missing: a test that quietly passes
    /// because its tooling is absent reads as coverage it does not provide.
    /// Run it explicitly and it either works or tells you to install busybox.
    #[test]
    #[ignore = "needs busybox on the host"]
    fn the_scripts_run_under_busybox() {
        use std::os::unix::fs::symlink;
        use std::process::Command;

        let busybox = which("busybox").expect(
            "busybox is not installed — `apt install busybox` (or run this on a host that has it)",
        );
        let root = std::env::temp_dir().join("verbinal-busybox-probe");
        let bin = root.join("bin");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&bin).expect("scratch dir");

        // Every applet BusyBox provides, as BusyBox provides it.
        let applets = Command::new(&busybox)
            .arg("--list")
            .output()
            .expect("busybox --list");
        for applet in String::from_utf8_lossy(&applets.stdout).lines() {
            let _ = symlink(&busybox, bin.join(applet.trim()));
        }
        // What the inspector image adds from apk, and so is NOT BusyBox.
        for real in ["bash", "python3"] {
            let path = which(real).unwrap_or_else(|| panic!("{real} is not installed"));
            let _ = std::fs::remove_file(bin.join(real));
            symlink(&path, bin.join(real)).expect("link");
        }

        for (script_name, script, env_var) in [
            ("probe.sh", probe_script(), "IMAGE_ID"),
            ("inspector.sh", inspector_script(), "TARGET_IMAGE"),
        ] {
            let dir = root.join(script_name);
            std::fs::create_dir_all(&dir).expect("home");
            let path = dir.join(script_name);
            std::fs::write(&path, script).expect("write script");

            // The inspector shells out to syft; stand in for it with output
            // shaped the way syft's syft-json is, so the transformer is
            // exercised rather than skipped.
            if script_name == "inspector.sh" {
                let stub = bin.join("syft");
                std::fs::write(
                    &stub,
                    "#!/usr/bin/env python3\nimport json\nprint(json.dumps({\
                     \"artifacts\":[{\"name\":\"musl\",\"version\":\"1.2.5-r0\",\"type\":\"apk\"}],\
                     \"distro\":{\"id\":\"alpine\",\"name\":\"Alpine Linux\",\
                     \"version\":\"3.20.3\",\"versionID\":\"3.20\",\
                     \"prettyName\":\"Alpine Linux v3.20\"},\
                     \"source\":{\"metadata\":{\"config\":{\"config\":{\"Env\":[]}}}}}))\n",
                )
                .expect("syft stub");
                let mut perms = std::fs::metadata(&stub).expect("stat").permissions();
                std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
                std::fs::set_permissions(&stub, perms).expect("chmod");
            }

            let out = Command::new(bin.join("bash"))
                .arg(&path)
                .env_clear()
                .env("PATH", &bin)
                .env("HOME", &dir)
                .env(env_var, "images.canfar.net/skaha/astroml:1.0")
                .output()
                .expect("run script");

            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            println!("--- {script_name} stderr ---\n{}", stderr.trim());

            assert!(
                !stderr.contains("unrecognized option") && !stderr.contains("Usage:"),
                "{script_name} used something BusyBox does not have:\n{stderr}"
            );
            let manifest: serde_json::Value =
                serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
                    panic!("{script_name} stdout is not a manifest ({e}): {stdout:.400}")
                });
            assert_eq!(
                manifest["imageID"], "images.canfar.net/skaha/astroml:1.0",
                "{script_name}"
            );
            assert!(
                manifest["probeNotes"].is_null(),
                "{script_name} gave up: {}",
                manifest["probeNotes"]
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Where an executable lives, or `None`.
    fn which(program: &str) -> Option<std::path::PathBuf> {
        std::env::var_os("PATH")?
            .to_string_lossy()
            .split(':')
            .map(|dir| std::path::Path::new(dir).join(program))
            .find(|path| path.is_file())
    }

    /// The Rust sanitiser and the scripts' `tr` must collapse the same set.
    ///
    /// They did not. Rust also mapped `@` and whitespace; the scripts did not.
    /// The two agree on every ordinary image id and diverge on exactly the
    /// character in a digest-pinned reference — so a reader built on the Rust
    /// rule would look for `image_sha256_…json` while the writer had published
    /// `image@sha256_…json`, and every lookup would miss with no error.
    #[test]
    fn the_sanitiser_matches_the_one_the_scripts_use() {
        for (name, script) in [
            ("probe.sh", probe_script()),
            ("inspector.sh", inspector_script()),
        ] {
            let line = script
                .lines()
                .find(|l| l.contains("tr '") && l.contains("_"))
                .unwrap_or_else(|| panic!("{name} no longer sanitises the image id"));
            // `tr '/:?*<>|"\\' '_'` — take what is between the first quotes.
            let set = line
                .split_once("tr '")
                .and_then(|(_, rest)| rest.split_once('\''))
                .map(|(set, _)| set)
                .unwrap_or_else(|| panic!("{name}: cannot read the tr set from {line}"));
            // The shell literal escapes the backslash; the char set holds one.
            let expected: String = SANITIZED.iter().collect();
            let shell: String = set.replace("\\\\", "\\");
            let mut a: Vec<char> = expected.chars().collect();
            let mut b: Vec<char> = shell.chars().collect();
            a.sort_unstable();
            b.sort_unstable();
            assert_eq!(a, b, "{name} collapses a different character set");
        }
    }

    #[test]
    fn a_manifest_path_is_the_one_the_scripts_write_to() {
        assert_eq!(
            manifest_path("images.canfar.net/skaha/astroml:1.0"),
            ".verbinal/manifests/images.canfar.net_skaha_astroml_1.0.json"
        );
    }

    /// Both scripts must publish a real content hash, not a marker.
    ///
    /// `inspector.sh` wrote the literal "sha256:syft", so a re-inspection could
    /// never answer "did anything change?" — the one question the field exists
    /// for. The stub branches keep the marker deliberately: a stub is rejected
    /// before anything looks at its hash.
    #[test]
    fn the_success_path_publishes_a_computed_content_hash() {
        assert!(
            inspector_script().contains(r#""contentHash": content_hash,"#),
            "inspector.sh is back to writing a marker instead of a hash"
        );
        assert!(
            inspector_script().contains("hashlib.sha256("),
            "inspector.sh no longer computes anything to publish"
        );
        // The in-container probe has always computed one, over marker files.
        assert!(probe_script().contains("sha256sum") || probe_script().contains("shasum"));
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex("hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(&sha256_hex("hello")[..12], "2cf24dba5fb0");
    }

    #[test]
    fn probe_name_is_content_hashed_stable_and_well_formed() {
        let name = probe_script_name();
        assert!(name.starts_with("probe-"), "got {name}");
        assert!(name.ends_with(".sh"), "got {name}");
        // "probe-" (6) + 12 hex + ".sh" (3) == 21
        assert_eq!(name.len(), 21, "got {name}");
        // Deterministic across calls.
        assert_eq!(name, probe_script_name());
    }

    #[test]
    fn inspector_name_is_well_formed_and_distinct_from_probe() {
        let name = inspector_script_name();
        assert!(name.starts_with("inspector-"), "got {name}");
        assert!(name.ends_with(".sh"), "got {name}");
        // "inspector-" (10) + 12 hex + ".sh" (3) == 25
        assert_eq!(name.len(), 25, "got {name}");
        // Different bodies -> different hashes.
        assert_ne!(
            &probe_script_name()[6..18],
            &inspector_script_name()[10..22]
        );
    }

    #[test]
    fn home_subdir_agrees_with_script_output_path() {
        assert_eq!(HOME_SUBDIR, ".verbinal");
        assert!(probe_script().contains(".verbinal"));
        assert!(inspector_script().contains(".verbinal"));
    }
}
