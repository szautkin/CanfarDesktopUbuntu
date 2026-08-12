use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SessionLaunchParams {
    pub name: String,
    pub image: String,
    pub session_type: String,
    pub cores: u32,
    pub ram: u32,
    pub gpus: u32,
    pub cmd: Option<String>,
    pub env: Option<String>,
    pub registry_username: Option<String>,
    pub registry_secret: Option<String>,
    /// Headless-job command arguments (each becomes a repeated `args` form field).
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Headless-job replica count ([`REPLICAS_RANGE`]); `None` for interactive
    /// sessions.
    #[serde(default)]
    pub replicas: Option<u32>,
}

/// How many identical headless replicas one launch may request.
///
/// One constant for the launch form's spin button, the `launch_headless_job`
/// schema and the applier's clamp. The reference's own two disagree — its UI
/// clamps to 20 while its MCP schema advertises 50 — and a schema that promises
/// more than the applier delivers is the widget-vs-schema divergence in reverse:
/// a client validates 40 as fine, sends it, and gets 20.
pub const REPLICAS_RANGE: (u32, u32) = (1, 20);

impl SessionLaunchParams {
    /// The fields every launch sends, under `name`.
    ///
    /// Shared by the interactive and headless builders so a field added to one
    /// cannot go missing from the other.
    fn base_form_pairs(&self, name: &str) -> Vec<(&'static str, String)> {
        // Cores/RAM are sent verbatim: a value of 0 means "platform-managed"
        // (flexible allocation), matching the reference's flexible-mode contract.
        let mut pairs = vec![
            ("name", name.to_string()),
            ("image", self.image.clone()),
            ("type", self.session_type.clone()),
            ("cores", self.cores.to_string()),
            ("ram", self.ram.to_string()),
        ];
        if self.gpus > 0 {
            pairs.push(("gpus", self.gpus.to_string()));
        }
        if let Some(ref cmd) = self.cmd {
            pairs.push(("cmd", cmd.clone()));
        }
        if let Some(ref args) = self.args {
            for arg in args {
                pairs.push(("args", arg.clone()));
            }
        }
        if let Some(ref env) = self.env {
            pairs.push(("env", env.clone()));
        }
        pairs
    }

    /// Form fields for an interactive session.
    ///
    /// No `replicas` here: replicas are a headless concept, and the field is
    /// carried by [`Self::headless_form_pairs`], which is the only path that
    /// knows how many POSTs there are.
    pub fn to_form_pairs(&self) -> Vec<(&'static str, String)> {
        self.base_form_pairs(&self.name)
    }

    /// Form fields for ONE replica of a headless job.
    ///
    /// Port of `Helpers/HeadlessRequestBuilder.cs`, which matches the wire shape
    /// the canonical opencadc/canfar Python client sends. Three things follow
    /// from that and none of them survived our single-POST version:
    ///
    /// * a replica set is N separate launches, each with its own name suffixed
    ///   `-1`, `-2`, … so the jobs are distinguishable in the session list;
    /// * every replica is told which one it is, through `REPLICA_ID` and
    ///   `REPLICA_COUNT` in its environment — the whole point of asking for
    ///   replicas is that each does a different slice of the work;
    /// * the `replicas` field goes only when the count exceeds one.
    pub fn headless_form_pairs(
        &self,
        replica_index: u32,
        replica_count: u32,
    ) -> Vec<(&'static str, String)> {
        let count = replica_count.max(1);
        let name = if count == 1 {
            self.name.clone()
        } else {
            format!("{}-{}", self.name, replica_index + 1)
        };

        let mut pairs = self.base_form_pairs(&name);
        pairs.push(("env", format!("REPLICA_ID={}", replica_index + 1)));
        pairs.push(("env", format!("REPLICA_COUNT={count}")));
        if count > 1 {
            pairs.push(("replicas", count.to_string()));
        }
        pairs
    }

    /// How many launches this set is: the replica count, at least one.
    pub fn replica_count(&self) -> u32 {
        self.replicas.unwrap_or(1).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_params() -> SessionLaunchParams {
        SessionLaunchParams {
            name: "test1".to_string(),
            image: "images.canfar.net/skaha/notebook:1.0".to_string(),
            session_type: "notebook".to_string(),
            cores: 2,
            ram: 8,
            gpus: 0,
            cmd: None,
            env: None,
            registry_username: None,
            registry_secret: None,
            args: None,
            replicas: None,
        }
    }

    #[test]
    fn form_pairs_basic() {
        let params = base_params();
        let pairs = params.to_form_pairs();
        assert_eq!(pairs.len(), 5);
        assert_eq!(pairs[0], ("name", "test1".to_string()));
        assert_eq!(
            pairs[1],
            ("image", "images.canfar.net/skaha/notebook:1.0".to_string())
        );
        assert_eq!(pairs[2], ("type", "notebook".to_string()));
        assert_eq!(pairs[3], ("cores", "2".to_string()));
        assert_eq!(pairs[4], ("ram", "8".to_string()));
    }

    #[test]
    fn form_pairs_with_gpus() {
        let mut params = base_params();
        params.gpus = 1;
        let pairs = params.to_form_pairs();
        assert_eq!(pairs.len(), 6);
        assert_eq!(pairs[5], ("gpus", "1".to_string()));
    }

    #[test]
    fn form_pairs_no_gpus_when_zero() {
        let params = base_params();
        let pairs = params.to_form_pairs();
        assert!(!pairs.iter().any(|(k, _)| *k == "gpus"));
    }

    #[test]
    fn form_pairs_with_cmd_and_env() {
        let mut params = base_params();
        params.cmd = Some("/bin/bash".to_string());
        params.env = Some("FOO=bar".to_string());
        let pairs = params.to_form_pairs();
        assert_eq!(pairs.len(), 7);
        assert!(pairs.contains(&("cmd", "/bin/bash".to_string())));
        assert!(pairs.contains(&("env", "FOO=bar".to_string())));
    }

    fn headless_params(replicas: u32) -> SessionLaunchParams {
        let mut params = base_params();
        params.name = "stack".to_string();
        params.session_type = "headless".to_string();
        params.cmd = Some("python".to_string());
        params.args = Some(vec!["run.py".to_string(), "--fast".to_string()]);
        params.replicas = Some(replicas);
        params
    }

    #[test]
    fn a_headless_replica_carries_the_command_line() {
        let pairs = headless_params(5).headless_form_pairs(0, 5);
        assert_eq!(pairs.iter().filter(|(k, _)| *k == "args").count(), 2);
        assert!(pairs.contains(&("args", "run.py".to_string())));
        assert!(pairs.contains(&("cmd", "python".to_string())));
    }

    #[test]
    fn each_replica_knows_which_one_it_is() {
        // The entire point of asking for replicas is that each does a different
        // slice of the work, and REPLICA_ID is how it finds out. Our single-POST
        // version sent neither.
        let params = headless_params(3);
        let second = params.headless_form_pairs(1, 3);
        assert!(second.contains(&("env", "REPLICA_ID=2".to_string())));
        assert!(second.contains(&("env", "REPLICA_COUNT=3".to_string())));
        // 1-based, like the reference: a job called "replica 0" reads as a bug.
        let first = params.headless_form_pairs(0, 3);
        assert!(first.contains(&("env", "REPLICA_ID=1".to_string())));
    }

    #[test]
    fn replicas_are_named_apart_but_a_lone_job_keeps_its_name() {
        let params = headless_params(3);
        assert!(params
            .headless_form_pairs(0, 3)
            .contains(&("name", "stack-1".to_string())));
        assert!(params
            .headless_form_pairs(2, 3)
            .contains(&("name", "stack-3".to_string())));

        // A single job is not "stack-1" — it is just "stack".
        let single = headless_params(1);
        assert!(single
            .headless_form_pairs(0, 1)
            .contains(&("name", "stack".to_string())));
    }

    #[test]
    fn the_replicas_field_goes_only_when_there_is_more_than_one() {
        let many = headless_params(4).headless_form_pairs(0, 4);
        assert!(many.contains(&("replicas", "4".to_string())));

        let single = headless_params(1).headless_form_pairs(0, 1);
        assert!(!single.iter().any(|(k, _)| *k == "replicas"));
    }

    #[test]
    fn an_interactive_launch_never_carries_replicas() {
        // Replicas are a headless concept, and the count belongs to the launcher
        // that knows how many POSTs it is making — not to a field riding along
        // on a single request, which is what made eight jobs into one.
        let params = headless_params(4);
        assert!(!params.to_form_pairs().iter().any(|(k, _)| *k == "replicas"));
    }

    #[test]
    fn the_replica_count_is_at_least_one() {
        let mut params = base_params();
        assert_eq!(params.replica_count(), 1, "an absent count means one job");
        params.replicas = Some(0);
        assert_eq!(params.replica_count(), 1, "zero jobs is not a launch");
        params.replicas = Some(6);
        assert_eq!(params.replica_count(), 6);
    }

    #[test]
    fn flexible_sends_zero_cores_and_ram() {
        let mut params = base_params();
        params.cores = 0;
        params.ram = 0;
        let pairs = params.to_form_pairs();
        assert!(pairs.contains(&("cores", "0".to_string())));
        assert!(pairs.contains(&("ram", "0".to_string())));
    }

    #[test]
    fn registry_credentials_not_in_form() {
        let mut params = base_params();
        params.registry_username = Some("user".to_string());
        params.registry_secret = Some("pass".to_string());
        let pairs = params.to_form_pairs();
        // Registry creds go in headers, not form data
        assert!(!pairs.iter().any(|(k, _)| k.contains("registry")));
    }
}
