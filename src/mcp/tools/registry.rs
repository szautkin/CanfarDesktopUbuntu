//! Registry tool family — reach images the platform does not list.
//!
//! `find_images_with_packages` searches what the app already knows about.
//! Everything here is about the images it does not: the container registry
//! behind Skaha holds far more than `/v1/image` publishes, and an agent asked
//! for "an image with CASA 6.5" should be able to look there rather than report
//! that the platform has none.
//!
//! Four tools, mirroring what the registry browser offers a person:
//!
//!   * `search_image_registry` — ask the registry. Read, but not free: it is a
//!     live call to an external service, so an empty term is refused rather
//!     than treated as "everything".
//!   * `list_my_images` — what has been added. Free, in-memory.
//!   * `add_registry_image` / `remove_registry_image` — change that list. Both
//!     go through the proposal pipeline; the removal is marked destructive,
//!     because from the user's side it takes away something they chose to keep.
//!
//! Adding an image puts it in [`AppServices::image_catalogue`], which is what
//! the images widget, the package search and the launch form all read — so an
//! agent that adds one has made it launchable, not just listed.

use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::tools::{opt_str_arg, str_arg, ToolDescriptor, ToolResult, VerbClass};
use crate::models::RegistryImage;
use crate::services::image_discovery_settings_service::ImageDiscoverySettingsService;
use crate::services::registry_service::RegistryAuth;
use crate::state::AppServices;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "search_image_registry".to_string(),
            description: "Search the container registry behind CANFAR (Harbor) for images the \
                platform's own catalogue does not list. Use this when find_images_with_packages \
                and the session-image list have nothing suitable — the registry holds far more \
                than Skaha publishes. `term` matches repository names (Harbor's own fuzzy search); \
                an empty term is REFUSED rather than returning the whole registry. `host` defaults \
                to the user's configured registry. Credentials come from the user's saved Harbor \
                CLI secret when there is one; public projects need none. Returns images[] of \
                {id, types}, where `types` are session types derived from the image's registry \
                labels and may be empty (such an image is still launchable via the Advanced launch \
                path, which takes an image reference directly). This is a live network call to a \
                shared service, and it is bounded — do not use it to enumerate the registry."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "term": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Repository name or fragment, e.g. 'casa' or 'astroml'."
                    },
                    "host": {
                        "type": "string",
                        "description": "Registry host, e.g. 'images.canfar.net'. Defaults to the \
                            user's configured registry host."
                    }
                },
                "required": ["term"],
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "list_my_images".to_string(),
            description: "List the registry images the user has added to their own image list. \
                Free — reads local state, no network. These appear alongside the platform's own \
                images everywhere in the app: the CANFAR Images list, find_images_with_packages, \
                and the launch form. Returns images[] of {id, types, addedAt} plus count."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "add_registry_image".to_string(),
            description: "Add a registry image to the user's own image list, making it available \
                throughout the app — the CANFAR Images list, find_images_with_packages, and the \
                launch form. Use the full reference from search_image_registry, e.g. \
                'images.canfar.net/skaha/astroml:24.07'. `types` are session types (notebook, \
                desktop, desktop-app, carta, headless, contributed); pass the ones \
                search_image_registry reported, or omit them — an image with no types is still \
                launchable via the Advanced launch path. Non-destructive: nothing is downloaded \
                and nothing runs; it only changes which images the app offers."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "image": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Full image reference, e.g. \
                            'images.canfar.net/skaha/astroml:24.07'."
                    },
                    "types": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Session types from the image's registry labels."
                    }
                },
                "required": ["image"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "remove_registry_image".to_string(),
            description: "Remove an image from the user's own image list. The image stays in the \
                registry and can be added again; what is lost is the user's choice to keep it, \
                which is why this is treated as destructive and is not auto-applied. Has no effect \
                on images the platform itself publishes — those cannot be removed."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "image": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Full image reference, as listed by list_my_images."
                    }
                },
                "required": ["image"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch
// ─────────────────────────────────────────────────────────────────────────────

/// Handle a registry call. `Some(..)` when `name` belongs to this family.
pub async fn dispatch(
    name: &str,
    services: &AppServices,
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    let result = match name {
        "search_image_registry" => search(services, args).await,
        "list_my_images" => list_mine(services),
        "add_registry_image" => propose_add(args, proposals),
        "remove_registry_image" => propose_remove(args, proposals),
        _ => return None,
    };
    Some(result)
}

async fn search(services: &AppServices, args: &Value) -> ToolResult {
    let term = str_arg(args, "term");
    if term.trim().is_empty() {
        return ToolResult::Failed("term is required".to_string());
    }

    // The user's own configuration, so an agent does not have to be told where
    // the registry is or who the user is on it.
    let settings = ImageDiscoverySettingsService::new();
    let host = opt_str_arg(args, "host")
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| settings.settings().registry_host.clone());
    let auth = RegistryAuth::from_basic(settings.current_auth_header());

    match services.registry.search(&host, &term, &auth).await {
        Ok(found) => {
            let added: std::collections::HashSet<String> = services
                .user_images
                .list()
                .into_iter()
                .map(|i| i.id)
                .collect();
            let images: Vec<Value> = found
                .iter()
                .map(|i| {
                    json!({
                        "id": i.id,
                        "types": i.types,
                        // So an agent does not propose adding what is already
                        // there, and can tell the user it is available now.
                        "alreadyAdded": added.contains(&i.id),
                    })
                })
                .collect();
            ToolResult::Data(json!({
                "host": host,
                "term": term,
                "count": images.len(),
                "images": images,
            }))
        }
        Err(e) => ToolResult::Failed(e),
    }
}

fn list_mine(services: &AppServices) -> ToolResult {
    let mine = services.user_images.list();
    let images: Vec<Value> = mine
        .iter()
        .map(|i| {
            json!({
                "id": i.id,
                "types": i.types,
                "addedAt": i.added_at,
            })
        })
        .collect();
    ToolResult::Data(json!({ "count": images.len(), "images": images }))
}

fn propose_add(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let image = str_arg(args, "image");
    if image.trim().is_empty() {
        return ToolResult::Failed("image is required".to_string());
    }
    let types = string_array(args, "types");
    let summary = format!("Add '{}' to your image list", image);
    let payload = json!({ "image": image, "types": types });
    ToolResult::Proposed(proposals.enqueue("add_registry_image", &summary, false, payload))
}

fn propose_remove(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let image = str_arg(args, "image");
    if image.trim().is_empty() {
        return ToolResult::Failed("image is required".to_string());
    }
    let summary = format!("Remove '{}' from your image list", image);
    let payload = json!({ "image": image });
    // Destructive: it takes away a choice the user made. The image itself is
    // untouched in the registry, but nothing here knows how they found it.
    ToolResult::Proposed(proposals.enqueue("remove_registry_image", &summary, true, payload))
}

/// A string array argument, absent or malformed reading as empty.
fn string_array(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────────────
// Apply
// ─────────────────────────────────────────────────────────────────────────────

pub async fn apply(
    services: &AppServices,
    proposal: &PendingProposal,
) -> Option<Result<String, String>> {
    match proposal.kind.as_str() {
        "add_registry_image" => Some(apply_add(services, &proposal.payload)),
        "remove_registry_image" => Some(apply_remove(services, &proposal.payload)),
        _ => None,
    }
}

fn apply_add(services: &AppServices, payload: &Value) -> Result<String, String> {
    let image = str_arg(payload, "image");
    if image.trim().is_empty() {
        return Err("image is required".to_string());
    }
    let types = string_array(payload, "types");
    services
        .user_images
        .add(RegistryImage::new(&image, &types))?;
    Ok(format!("Added '{image}' to your image list."))
}

fn apply_remove(services: &AppServices, payload: &Value) -> Result<String, String> {
    let image = str_arg(payload, "image");
    if image.trim().is_empty() {
        return Err("image is required".to_string());
    }
    services.user_images.remove(&image)?;
    Ok(format!("Removed '{image}' from your image list."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_family_advertises_both_halves_of_the_workflow() {
        // An agent that can search but not add has found something it cannot
        // act on; one that can add but not list cannot tell what it has done.
        let names: Vec<String> = descriptors().into_iter().map(|d| d.name).collect();
        for expected in [
            "search_image_registry",
            "list_my_images",
            "add_registry_image",
            "remove_registry_image",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn searching_the_registry_is_a_read_and_changing_the_list_is_a_write() {
        for d in descriptors() {
            let expected = match d.name.as_str() {
                "search_image_registry" | "list_my_images" => VerbClass::Read,
                _ => VerbClass::Write,
            };
            assert_eq!(d.verb, expected, "{} has the wrong verb", d.name);
        }
    }

    #[test]
    fn a_blank_image_is_refused_rather_than_queued() {
        // A proposal with no image applies to nothing and can only fail later,
        // after the user has approved it.
        let store = Arc::new(InMemoryProposalStore::new());
        for blank in ["", "   "] {
            match propose_add(&json!({ "image": blank }), &store) {
                ToolResult::Failed(m) => assert!(m.contains("image is required"), "{m}"),
                _ => panic!("a blank image was accepted by add"),
            }
            match propose_remove(&json!({ "image": blank }), &store) {
                ToolResult::Failed(m) => assert!(m.contains("image is required"), "{m}"),
                _ => panic!("a blank image was accepted by remove"),
            }
        }
    }

    #[test]
    fn an_empty_term_is_refused_before_any_network_call() {
        // Harbor reads `q=` as "everything", and a tool call is exactly where a
        // stray empty string comes from. `search` needs an AppServices to run,
        // so this asserts the guard is in the code path rather than building
        // the whole app to watch it not make a request; the service layer has
        // the executable version of this test.
        let code =
            crate::testing::without_comments(crate::testing::code(include_str!("registry.rs")));
        let at = code
            .find("async fn search(")
            .expect("search is gone from the registry family");
        let body = &code[at..(at + 400).min(code.len())];
        assert!(
            body.contains("term.trim().is_empty()"),
            "the registry search no longer refuses an empty term"
        );
    }

    #[test]
    fn removing_asks_first() {
        // It takes away something the user chose to keep. Auto-applying that
        // because it is "only a list entry" is the app deciding for them.
        let store = Arc::new(InMemoryProposalStore::new());
        match propose_remove(&json!({ "image": "h/p/n:1" }), &store) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "remove_registry_image");
                assert!(p.destructive, "removal is auto-applied without review");
                assert_eq!(p.payload["image"], "h/p/n:1");
            }
            _ => panic!("remove did not queue a proposal"),
        }
    }

    #[test]
    fn adding_does_not_ask_because_nothing_is_lost() {
        let store = Arc::new(InMemoryProposalStore::new());
        match propose_add(
            &json!({ "image": "h/p/n:1", "types": ["notebook"] }),
            &store,
        ) {
            ToolResult::Proposed(p) => {
                assert!(!p.destructive);
                assert_eq!(p.payload["types"][0], "notebook");
            }
            _ => panic!("add did not queue a proposal"),
        }
    }

    #[test]
    fn a_proposal_carries_everything_apply_needs() {
        // Propose and apply read the payload through the same accessors, so a
        // renamed key cannot make an accepted proposal fail on execution.
        let store = Arc::new(InMemoryProposalStore::new());
        let ToolResult::Proposed(p) =
            propose_add(&json!({ "image": "h/p/n:1", "types": ["carta"] }), &store)
        else {
            panic!("no proposal");
        };
        assert_eq!(str_arg(&p.payload, "image"), "h/p/n:1");
        assert_eq!(string_array(&p.payload, "types"), vec!["carta"]);
    }

    #[test]
    fn malformed_types_read_as_none_rather_than_failing_the_call() {
        // An agent that sends a string where an array belongs should get an
        // image with no types, not a rejected call — the image is the point.
        assert!(string_array(&json!({ "types": "notebook" }), "types").is_empty());
        assert!(string_array(&json!({}), "types").is_empty());
        assert_eq!(
            string_array(&json!({ "types": ["a", 2, "b"] }), "types"),
            vec!["a", "b"]
        );
    }
}
