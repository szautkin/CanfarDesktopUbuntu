//! Searching the container registry behind the platform.
//!
//! Skaha's `/v1/image` is a curated list; the registry behind it holds far
//! more. This is how someone reaches an image the platform has not listed —
//! a colleague's build, a tag Skaha has not picked up — and it runs only when
//! they ask. Nothing here is called on a timer or at start-up: enumerating a
//! Harbor instance to populate a dashboard card would be a great deal of
//! traffic to answer a question nobody asked.
//!
//! Harbor's own API rather than the OCI distribution API, for one reason:
//! labels. `/v2/_catalog` and `/v2/<name>/tags/list` will enumerate a registry
//! anywhere, but neither reports labels without pulling each image's config
//! blob, and the labels are what say whether an image is a notebook or a CARTA
//! session. Without them an added image cannot be typed, cannot be filtered in
//! the widget, and cannot be offered on the Standard launch tab.
//!
//! Two calls per search:
//!
//!   1. `GET /api/v2.0/search?q=<term>` — Harbor's own fuzzy search, which is
//!      what makes this a search rather than a download of everything.
//!   2. `GET /api/v2.0/projects/<p>/repositories/<r>/artifacts?with_label=true`
//!      for each repository it returned, for the tags and labels.
//!
//! Step 2 is the expensive one, so it is bounded twice over: at most
//! [`MAX_REPOSITORIES`] repositories are opened at all, and at most
//! [`CONCURRENT_REPOSITORY_READS`] of them at a time. An unbounded fan-out here
//! would be this app deciding, on one keystroke, to open fifty connections to a
//! shared service.

use crate::models::RegistryImage;
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;

/// How many repositories one search will open.
///
/// Harbor's search happily returns hundreds for a short term. Past the first
/// couple of dozen the user is not reading results, they are re-typing their
/// search — so the rest is traffic spent on a list nobody scrolls.
pub const MAX_REPOSITORIES: usize = 24;

/// How many of those are read at once.
pub const CONCURRENT_REPOSITORY_READS: usize = 4;

/// How many artifacts to take from one repository.
///
/// Newest first, which is Harbor's default order: a repository with two hundred
/// tags is a build history, and the user is looking for the current one.
const ARTIFACTS_PER_REPOSITORY: usize = 10;

/// How long one registry call may take.
const TIMEOUT_SECS: u64 = 20;

// ── Harbor's wire shapes, only the fields used ──────────────────────────────

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    repository: Vec<SearchRepository>,
}

#[derive(Deserialize)]
struct SearchRepository {
    project_name: String,
    /// Fully qualified as `project/name`, and `name` may itself contain `/`.
    repository_name: String,
}

#[derive(Deserialize)]
struct Artifact {
    #[serde(default)]
    tags: Vec<Tag>,
    #[serde(default)]
    labels: Vec<Label>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
}

#[derive(Deserialize)]
struct Label {
    name: String,
}

/// What to talk to, and as whom.
///
/// Credentials are optional: a public project answers without them, and the
/// browser lets someone try a search before going to find their CLI secret.
#[derive(Debug, Clone, Default)]
pub struct RegistryAuth {
    /// `base64(username:secret)`, ready for a Basic header. The same value the
    /// discovery settings already mint for `x-skaha-registry-auth`, so a user
    /// who configured discovery has already configured this.
    pub basic: Option<String>,
}

impl RegistryAuth {
    pub fn from_basic(basic: Option<String>) -> Self {
        RegistryAuth { basic }
    }

    /// Credentials typed into the browser for this search only, never stored.
    pub fn from_credentials(username: &str, secret: &str) -> Self {
        if username.trim().is_empty() || secret.is_empty() {
            return RegistryAuth::default();
        }
        let raw = format!("{}:{}", username.trim(), secret);
        RegistryAuth {
            basic: Some(base64::engine::general_purpose::STANDARD.encode(raw)),
        }
    }
}

pub struct RegistryService {
    client: Client,
}

impl RegistryService {
    pub fn new(client: Client) -> Self {
        RegistryService { client }
    }

    /// Images in `host` whose repository matches `term`.
    ///
    /// Returns them newest-repository-first, each with the tags and labels
    /// Harbor reports. An empty term is refused rather than treated as "match
    /// everything": that is the whole-registry download this module exists to
    /// avoid.
    pub async fn search(
        &self,
        host: &str,
        term: &str,
        auth: &RegistryAuth,
    ) -> Result<Vec<RegistryImage>, String> {
        let host = host.trim().trim_end_matches('/');
        if host.is_empty() {
            return Err("No registry host configured.".into());
        }
        let term = term.trim();
        if term.is_empty() {
            return Err("Type something to search for.".into());
        }

        let url = format!(
            "https://{host}/api/v2.0/search?q={}",
            urlencoding::encode(term)
        );
        let found: SearchResponse = self.get_json(&url, auth).await?;

        let repositories: Vec<SearchRepository> = found
            .repository
            .into_iter()
            .take(MAX_REPOSITORIES)
            .collect();

        // Bounded fan-out. Each chunk is awaited before the next starts, so at
        // most CONCURRENT_REPOSITORY_READS requests are ever in flight — this
        // app should never be the reason a shared registry is busy.
        let mut images = Vec::new();
        for chunk in repositories.chunks(CONCURRENT_REPOSITORY_READS) {
            let mut set = tokio::task::JoinSet::new();
            for repo in chunk {
                let url = artifacts_url(host, &repo.project_name, &repo.repository_name);
                let name = repo.repository_name.clone();
                let client = self.client.clone();
                let auth = auth.clone();
                let host = host.to_string();
                set.spawn(async move {
                    let artifacts = fetch_json::<Vec<Artifact>>(&client, &url, &auth)
                        .await
                        .unwrap_or_default();
                    to_images(&host, &name, &artifacts)
                });
            }
            while let Some(joined) = set.join_next().await {
                // A repository that fails to read is skipped, not fatal: a
                // search across projects will routinely include one the user
                // cannot see, and losing the other twenty-three to it would be
                // the wrong answer.
                if let Ok(mut found) = joined {
                    images.append(&mut found);
                }
            }
        }

        images.sort_by(|a, b| a.id.cmp(&b.id));
        images.dedup_by(|a, b| a.id == b.id);
        Ok(images)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        auth: &RegistryAuth,
    ) -> Result<T, String> {
        fetch_json(&self.client, url, auth).await
    }
}

async fn fetch_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    url: &str,
    auth: &RegistryAuth,
) -> Result<T, String> {
    let mut request = client
        .get(url)
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS));
    if let Some(basic) = &auth.basic {
        request = request.header(reqwest::header::AUTHORIZATION, format!("Basic {basic}"));
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("Could not reach the registry: {e}"))?;

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        // Named specifically because the fix is specific, and because the CADC
        // password is the wrong answer often enough to be worth saying.
        return Err(
            "The registry rejected these credentials. Use your Harbor CLI secret, \
             not your CADC password."
                .into(),
        );
    }
    if !status.is_success() {
        return Err(format!("The registry returned HTTP {}.", status.as_u16()));
    }

    response
        .json::<T>()
        .await
        .map_err(|e| format!("Could not read the registry's answer: {e}"))
}

/// Harbor's artifacts URL for one repository.
///
/// The repository segment is double-encoded on purpose. Harbor addresses a
/// nested repository (`skaha/base/astro`) as one path segment, so its `/` must
/// survive the proxy's decode and arrive at Harbor still encoded — hence
/// encoding the already-encoded form.
fn artifacts_url(host: &str, project: &str, repository_name: &str) -> String {
    let bare = repository_name
        .strip_prefix(&format!("{project}/"))
        .unwrap_or(repository_name);
    let encoded = urlencoding::encode(&urlencoding::encode(bare)).into_owned();
    format!(
        "https://{host}/api/v2.0/projects/{}/repositories/{encoded}/artifacts\
         ?with_label=true&page_size={ARTIFACTS_PER_REPOSITORY}&page=1",
        urlencoding::encode(project)
    )
}

/// One repository's artifacts as launchable image references.
///
/// An untagged artifact is skipped: it can only be addressed by digest, which
/// is not what the launch form takes, so listing it would offer something that
/// cannot be launched.
fn to_images(host: &str, repository_name: &str, artifacts: &[Artifact]) -> Vec<RegistryImage> {
    let mut out = Vec::new();
    for artifact in artifacts {
        let labels: Vec<String> = artifact.labels.iter().map(|l| l.name.clone()).collect();
        for tag in &artifact.tags {
            out.push(RegistryImage::new(
                format!("{host}/{repository_name}:{}", tag.name),
                &labels,
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nested_repository_survives_the_proxy() {
        // Harbor treats "base/astro" as ONE repository whose name contains a
        // slash. Encoded once, the proxy decodes it back to a slash and the
        // request addresses a path that does not exist.
        let url = artifacts_url("images.canfar.net", "skaha", "skaha/base/astro");
        assert!(
            url.contains("/repositories/base%252Fastro/"),
            "nested repository name is not double-encoded: {url}"
        );
        assert!(!url.contains("/repositories/base/astro/"));
    }

    #[test]
    fn the_project_prefix_is_not_repeated() {
        // Harbor's search reports `repository_name` fully qualified, but the
        // artifacts URL already names the project. Sending it twice asks for
        // `skaha/skaha/base`, which is a 404.
        let url = artifacts_url("h", "skaha", "skaha/base");
        assert!(url.contains("/projects/skaha/repositories/base/"), "{url}");
    }

    #[test]
    fn a_repository_that_is_not_prefixed_is_left_alone() {
        let url = artifacts_url("h", "skaha", "base");
        assert!(url.contains("/repositories/base/"), "{url}");
    }

    #[test]
    fn labels_ride_along_to_every_tag_of_an_artifact() {
        let artifacts = vec![Artifact {
            tags: vec![
                Tag {
                    name: "24.01".into(),
                },
                Tag {
                    name: "latest".into(),
                },
            ],
            labels: vec![Label {
                name: "notebook".into(),
            }],
        }];
        let images = to_images("h", "skaha/astro", &artifacts);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].id, "h/skaha/astro:24.01");
        assert_eq!(images[1].id, "h/skaha/astro:latest");
        assert!(images.iter().all(|i| i.types == vec!["notebook"]));
    }

    #[test]
    fn an_untagged_artifact_is_skipped() {
        // Addressable only by digest, which the launch form does not take.
        // Listing it would offer something that cannot be launched.
        let artifacts = vec![Artifact {
            tags: vec![],
            labels: vec![Label {
                name: "notebook".into(),
            }],
        }];
        assert!(to_images("h", "p/r", &artifacts).is_empty());
    }

    #[tokio::test]
    async fn an_empty_search_is_refused_rather_than_matching_everything() {
        // Harbor reads `q=` as "everything". That is the whole-registry
        // download this module exists to avoid, and it is one stray Enter away.
        let svc = RegistryService::new(Client::new());
        let err = svc
            .search("images.canfar.net", "   ", &RegistryAuth::default())
            .await
            .unwrap_err();
        assert!(err.contains("Type something"), "{err}");
    }

    #[tokio::test]
    async fn a_search_with_no_host_says_so_rather_than_building_a_bad_url() {
        let svc = RegistryService::new(Client::new());
        assert!(svc
            .search("  ", "astro", &RegistryAuth::default())
            .await
            .is_err());
    }

    #[test]
    fn typed_credentials_only_count_when_both_halves_are_there() {
        assert!(RegistryAuth::from_credentials("", "secret").basic.is_none());
        assert!(RegistryAuth::from_credentials("me", "").basic.is_none());
        // base64("me:secret")
        assert_eq!(
            RegistryAuth::from_credentials("me", "secret")
                .basic
                .unwrap(),
            "bWU6c2VjcmV0"
        );
    }

    #[test]
    fn a_search_opens_a_bounded_number_of_connections() {
        // This app should never be the reason a shared registry is busy. The
        // Portal has already overloaded a shared node once by fanning out
        // without a bound. Compile-time, because both operands are consts.
        const {
            assert!(CONCURRENT_REPOSITORY_READS <= 4, "search fans out too far");
            assert!(MAX_REPOSITORIES <= 50, "one search opens too many repos");
        };
    }
}
