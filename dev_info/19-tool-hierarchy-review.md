# 19 — Review: family-aware tool descriptions

A review of the MCP tool-hierarchy plan, checked against the tree and the
running app rather than against the plan's own description of them.

**Verdict: the problem is real and well-quantified. The proposed mechanism does
not solve it, and would make it marginally worse.** The lever the plan needs is
already in the codebase and the plan does not mention it. Everything below is
either a measurement or a file you can open.

---

## What the plan gets right

Measured against the live app, over its own socket:

```
tool count:        147
tools/list bytes:  96,434  (~24,000 tokens)
  descriptions     39,565 B
  schemas          46,547 B
  names             2,487 B
```

So "147 tools" and "tens of thousands of tokens" are both accurate, and the cost
is worth attacking. `describe_app` really is monolithic (`read.rs:44`, an
`empty_schema()` and a two-line summary), and the `server.rs` note about
re-registering tools thrashing a client really is there.

Two measurements the plan does not have, and should:

- **The schemas outweigh the descriptions**, and the weight is concentrated. The
  five heaviest tools are all `set_*`; `set_search_form` alone is 3,826 bytes,
  and **the ten heaviest tools are 23% of the entire payload**. The median tool
  is 483 bytes.
- That concentration is what makes both the cheap fix and the real fix
  tractable: ten tools are a quarter of the cost, and a default set of ten
  median-sized tools would be about **5 KB against today's 96 KB** — a 95%
  reduction, better than the 80% the plan claims for a mechanism that delivers
  none.

---

## Three factual corrections

**1. `src/mcp/tools/catalog.rs` already exists.** It is 37 KB and it is the
router/catalog assembly ported from `McpToolCatalog.cs`. The plan proposes
creating a new module at that exact path. Pick another name.

**2. `family_descriptors()` does not return families.** The plan's central
premise —

> The grouping already exists internally … `family_descriptors()` returns 13
> families … So the data is there; only the MCP surface is missing.

— is wrong. It returns a **flat `Vec<ToolDescriptor>`**: thirteen
`v.extend(x::descriptors())` calls concatenated. There is no family id, no
title, no description, and nothing at runtime that knows which tool came from
which module. The grouping exists only as *module boundaries in source*, which
is not queryable.

This is not a detail. It means the work is not "expose what we have" but "build
a taxonomy, then keep it true" — a different and larger job, and one with a
maintenance cost the plan does not budget for.

**3. A grouping DOES already exist — in the UI.**
`ai_guide_page::category_id_for_tool()` maps every one of the 147 tools to one
of **16 categories** (foundational, fits, cube, notebook, storage, sessions,
workflows, search, research, discovery, queries, compute, control, downloads,
guide, headless), and `every_tool_lands_in_a_real_category` fails the build if a
tool is added without one.

§4 of the plan proposes a *second* taxonomy of 10 apps. Two groupings of the
same 147 tools will drift, and not slowly: in this codebase a duplicated
extension→kind mapping drifted **within a day** — `.txt` became openable and
`list_notebooks` still reported it as `"other"`. The fix there was to derive one
from the other. Same answer here.

---

## The design problem: adding tools does not remove tools

This is the part that decides whether the plan ships.

A standard MCP client calls `tools/list` once and puts the result in context.
There is no negotiation, and no convention that lets a server say "ignore that,
call `list_apps` instead". So after this change the client receives:

| | tools | tokens |
| --- | --- | --- |
| today | 147 | ~24,000 |
| after the plan | **151** | **~24,500** |

The plan acknowledges this in §9 — *"Only helps clients that lazy-load"* — and
in §5.5 proposes a client-convention note. But a note cannot change how Claude
Desktop or Claude Code behave, and the goal in §1 is an **80% reduction**. As
specified, the reduction is negative.

The stated acceptance criterion makes the same assumption:

> A client using only `list_apps`+`describe_app` can accomplish a FITS-only task
> with **no** call to `tools/list`.

A client cannot choose not to call `tools/list`; that is the first thing it
does. This criterion cannot be met by any client the app actually talks to.

---

## The lever the plan is missing

**The server already supports a dynamic tool list, and the plan does not mention
it.** `src/mcp/server.rs`:

- `"capabilities": { "tools": { "listChanged": true } }` (line 355)
- it emits `notifications/tools/list_changed` (line 152)
- a burst of changes is debounced into one notification, with a test
  (`a_burst_of_changes_is_one_notification`)

That is exactly the machinery progressive disclosure needs, already built and
already guarded. The server decides what `tools/list` returns, so:

1. Advertise a **small default set** — the catalog tools plus the genuinely
   cross-cutting ones (auth, health, current view, job status: the
   `foundational` category, which already exists).
2. `open_app("fits")` registers that family's tools and fires `list_changed`.
3. The client re-reads `tools/list` and now sees the FITS tools.

This actually delivers the 80%, because the reduction happens in the payload the
client is obliged to read. And a tool that is not advertised can still be
*called*, so a client that guesses a name is not blocked — the failure mode is
soft.

**The risk is real and belongs in the plan:** dynamic lists surprise clients that
cache, `advertised_names_match_the_reference` compares the advertised set against
the reference and would need to know which mode it is in, and the AI Guide reads
`all_live_descriptors()` and must keep showing everything. None of these is
hard; all of them are invisible until they break.

---

## A cheaper first move the plan skips

Before any protocol work: **46.5 KB of the 96 KB is input schemas**, and the
worst offenders are a handful of `set_*` tools. That is worth one afternoon of
measurement — not blind trimming, because a description is what makes a tool
selectable, but a 3,826-byte schema for one tool is worth *looking* at.

This has no protocol risk, no client-behaviour assumption, and no new tools. It
should be step zero, if only because it establishes the measurement harness the
rest of the work will be judged by.

---

## Smaller notes

- **`tool(name)` earns little.** §10 Q2 already recommends inlining schemas into
  `describe_app(name)`, which makes `tool(name)` redundant. Two tools added to
  solve a too-many-tools problem is a trade worth refusing.
- **`search_tools` is the strongest of the four.** It is the only one that helps
  an agent that does not know the taxonomy, which is the common case. If only
  one thing ships, ship this.
- **"total tool counts sum to 147" is a bad acceptance criterion.** That number
  changes on every release — this session alone added three tools. A test that
  must be edited whenever anyone adds a tool is a test that gets deleted. Assert
  the *invariant*: every tool appears in exactly one app, and the union equals
  `family_descriptors()`.
- **The missing test is the one that matters.** None of §7 checks that the
  taxonomy stays true as tools are added. That is the only failure that will
  actually happen. Model it on `every_tool_lands_in_a_real_category`, which
  already does this for the UI grouping.

---

## What I would do instead

Phased, so each step stands alone and the risky one comes last.

**Phase 0 — measure.** A probe that reports `tools/list` size broken down by
tool and by category, so every later claim is checked rather than asserted.
Ten tools carry 23% of the payload, so this is a short list to review, not a
sweep. Trim what is indefensible; leave what earns its place. No protocol
change, and it gives every later phase a number to be judged against.

**Phase 1 — one taxonomy, shared.** Move `category_id_for_tool` out of
`ai_guide_page` into a model both layers read. The AI Guide keeps working; the
MCP layer gains a grouping without a second table to keep true. Extend
`describe_app` to take an optional `{app}` and return that category's tools with
schemas. This is backward compatible and useful on its own — an agent can ask
"what can you do with FITS?" and get an answer.

**Phase 2 — `search_tools`.** Cheap, independently valuable, no assumptions
about client behaviour.

**Phase 3 — progressive disclosure, behind a setting.** Default off. When on,
advertise `foundational` + the catalog, and register a family on
`open_app(name)` via the `listChanged` machinery that already exists. Off by
default because it changes what every client sees, and the app has already been
bitten once by re-registering tools under a client.

Phases 0–2 deliver most of the discoverability benefit with none of the
protocol risk. Phase 3 is where the token reduction actually lives, and it is
the one to hold until the others are in and measured.

---

## Answers to the plan's open questions

**1. Adopt §4's taxonomy?** No — adopt the 16 categories that already exist and
are already guarded. A second taxonomy is a maintenance liability with no
compensating benefit, and the names have already been reviewed once.

**2. Inline schemas in `describe_app(name)`?** Yes, and drop `tool(name)`. A
category is ~10–15 tools; that is a few thousand tokens, which is the point.

**3. Deprecate `tools/list`?** Never. It is the protocol. Even under Phase 3 it
stays complete for any client that wants everything — the difference is what is
advertised by default, not what is reachable.
