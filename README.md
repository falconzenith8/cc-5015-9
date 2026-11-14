# Procedural Virtual Plant Studio

A deterministic procedural plant generator written in Rust. Given a parameter
set it grows a trunk, recursively places branches on the parent branches'
curved centerlines, and instances leaf blades — all from real tube/leaf
geometry, never a pre-made model or particle billboard.

## Workspace layout

| Crate | Role | Third-party deps |
| --- | --- | --- |
| `plant-core` | Deterministic generation, math, geometry, validation, export, rule DSL + migration | none |
| `plant-cli` | Headless generation / validation / stress / export / rule migration | `plant-core` |
| `plant-gui` | Realtime `egui`/`eframe` rule & parameter editor with orbit viewport | `eframe`, `egui` |

## Rule DSL and the v1 -> v2 migration

Plants can be described by text rule documents in two syntaxes. Both compile
to the same internal `RulePlan` and run through the **single** deterministic
builder — there is no second generator.

### Legacy `plant-rules/v1` — implicit

```text
plant-rules/v1
unit cm                 # optional; absent => bare lengths mean metres
seed 777
trunk length 320        # bare number => default length unit (cm here)
branch angle 0.5        # bare number => default angle unit (radians)
material bark trunk     # positional branch reference / layer word
attach parent 0.4
```

### Explicit `plant-rules/v2` — no implicit defaults

```text
plant-rules/v2
seed value=777
trunk length=320cm radius=22cm
branch count=4 angle=0.5rad ratio.length=0.68 ...
material name=bark layer=branch:trunk
attach target=branch:parent at=0.4
```

* Every dimensional value carries a unit suffix: lengths `m`/`cm`/`mm`,
  angles `rad`/`deg`. A bare number where a unit belongs is a **hard parse
  error**, never a silently applied default. Dimensionless ratios stay plain
  fractions under `ratio.*` / `taper` / `bend` / `lift`.
* Branch references and material layers are qualified selectors:
  `branch:trunk`, `branch:<depth>`, `branch:<a>..`, `branch:<a>..<b>`,
  `leaves`; attach targets use `branch:trunk` / `branch:parent`.

### The converter's contract

`rule-migrate` converts v1 -> v2 deterministically and never hides a problem
behind a default:

* the 64-bit **seed**, the **branching levels** (count, depth, ratios) and
  every **material binding** are preserved;
* each implicit default the legacy parser used is written out explicitly and
  listed in the report and in the converted file's header comments;
* a stated value equal to an engine default is still treated as *stated*
  (presence is tracked explicitly, so explicit input is never mislabeled);
* rules with no v2 equivalent are **pending**: kept verbatim as comments and
  returned in the report. `--strict` makes any pending rule fail the command;
* the original project file is copied to `<file>.v1.bak` before writing — a
  rollback copy always exists.

Equivalence is decided on **real generated geometry**, not on seed strings:
`rule-verify` builds the plant through both rule paths and compares node
counts, depths, spline samples, world transforms, radii and leaf
anchors/normals field-by-field. A same-seed/different-rules pair is asserted
to diverge, so an identical seed string is never accepted as cross-algorithm
equality. Material semantics are checked against actual layer identity (node
depth) and measured world-space extents (`rule-bindings.json`).

Every converted file and report carries the pinned migration marker
`migration-version: rule-migration/v1-to-v2` (also in `Cargo.toml`
`[workspace.metadata.rule_migration]` and the `MIGRATION_VERSION` constant).

## Design guarantees

### 1. Reproducible rules — no global random state

There is no `thread_rng` or process-global RNG anywhere. Every random stream
is an explicit `plant_core::rng::Rng` (SplitMix64) constructed from a 64-bit
seed. Node seeds are a pure function of the parent seed, child index and root
seed (`builder::mix_child_seed`), so the same `PlantParams` always produces
the identical tree — across processes and machines. Verified byte-for-byte:

```text
$ plant generate --seed 99 ... --out-dir run-a
$ plant generate --seed 99 ... --out-dir run-b
$ cmp run-a/plant.json run-b/plant.json   # identical
```

### 2. Correct local-coordinate composition

Each `BranchNode` stores a local TRS transform relative to its parent and a
cached world transform. `validation::check_transforms` recomputes every world
matrix from the local chain and asserts equality. Children attach to
**discrete samples of the parent's curved spline** (not interpolated points),
so after a parent is bent or scaled the child base still sits exactly on the
parent centerline — enforced numerically by `check_attachments`.

### 3. Hard limits — no unbounded recursion or allocation

The builder is an iterative BFS loop, never recursion. Every request is
clamped in `PlantParams::validated` against absolute ceilings
(`HARD_MAX_DEPTH/ NODES / LEAVES / VERTICES / TRIANGLES / SEGMENTS / SIDES`).
Exhausted budgets truncate emission deterministically and record a
`PruneReason` (`DepthLimit`, `NodeBudget`, `Degenerate`); they never panic.
Example: requesting 100,000 branches with `--max-nodes 50` terminates in
~2.6 s with exactly 50 nodes and a prune count explaining the rest.

### 4. Real geometric checks, not heuristics

* **Trunk seams** — tubes are built ring-by-ring with shared edge topology;
  `check_tube_integrity` verifies every ring pair is fully connected and no
  triangle is zero-area or non-finite.
* **Branch degeneration** — branches below length/radius floors are pruned
  with reason `Degenerate`; `is_degenerate` also measures actual centerline
  arc length. Validation asserts no degenerate node reaches the output.
* **Leaf orientation** — each leaf instance carries a world normal;
  `check_leaf_orientation` asserts it is unit length, matches the baked
  transform's Z axis, and that the anchor matches the transform origin.
* **No occlusion culling, by construction** — nothing ever deletes a branch
  for being hidden. A dedicated dense-overlap test
  (`no_occlusion_based_deletion_even_with_dense_overlap`) pins this.

### 5. Structure separated from render instances

The node tree (`Plant::nodes`, hierarchy + local/world transforms + splines +
seeds) is distinct from geometry: per-node baked tube meshes and a shared leaf
blade mesh referenced by explicit `LeafInstance { world, anchor, normal }`
transforms. The GUI and OBJ writer consume instances; the hierarchy stays
editable.

### 6. Export preserves or bakes hierarchy explicitly

* `params.json` — parameter archive (`plant-params/v1`); re-feeding it to the
  builder regenerates the identical plant.
* `plant.json` — full editable hierarchy (`plant-hierarchy/v1`): every node's
  parent id, local/world matrices, spline samples, seed, children and every
  leaf instance, plus the prune log. Re-import returns an equal `Plant`.
* `plant.obj` + `plant.obj.manifest.json` — baked flattened geometry whose
  `g branch_<id>` / `g leaf_<n>` groups map back to node ids via the sidecar.
* `rule-bindings.json` — only for `rule-generate`: the rule's material
  bindings plus geometry evidence (matched node/leaf ids and world-space
  bounds), making layer identity and geometric extent reviewable per build.

## Usage

```bash
# Build
cargo build --release

# Generate with realtime-relevant parameters
./target/release/plant generate \
  --seed 1337 --branches 3 --angle 0.7 --length-ratio 0.72 \
  --bend 0.08 --depth 6 --out-dir out

# Re-validate an exported hierarchy (all geometric checks)
./target/release/plant validate out/plant.json

# Rebuild from a parameter archive and check in-process determinism
./target/release/plant regenerate out/params.json --out-dir rebuilt

# Required edge-case suite: fixed seed, zero branches, sharp angle,
# many small leaves, tiny budget — prints budget reports and writes artifacts
./target/release/plant stress --out-dir stress

# Rule workflow (v1 legacy -> v2 explicit). The first command writes a
# <file>.v1.bak rollback copy plus a *.migration.json report.
./target/release/plant rule-migrate shrub.legacy.rules \
    --out shrub.v2.rules --report shrub.migration.json [--strict]
./target/release/plant rule-verify shrub.legacy.rules            # geometry proof
./target/release/plant rule-generate shrub.v2.rules --out-dir out # build from rules
```

### GUI

```bash
cargo run --release -p plant-gui
```

The window opens with two surfaces. The left panel has the classic parameter
sliders **and** a "Rule document" editor: paste or load a `plant-rules/v1` or
`plant-rules/v2` file, press **Apply rule & rebuild** to drive the core
through the rule path, or **Migrate v1 -> v2** to convert and immediately
rebuild on the new implementation. Material/layer evidence (matched node ids
and world-space extents) renders under the editor; any parse or migration
failure is returned into the panel's status line and leaves the current plant
intact. The right panel is the orbit-drag / scroll-zoom viewport drawing the
actual generated splines and leaf instances. A native display (Wayland/X11)
is required; on a headless host the binary exits with a clear window-system
error. The viewport-free engine behind the panel is covered by `plant-gui`
unit tests, so the rule workflow is verifiable without a display.

## Test matrix

`cargo test` runs 52 tests, including the required scenarios:

* **fixed seed** — two independent builds compare equal field-for-field;
  cross-process export is byte-identical.
* **zero branches** — trunk-only plant, one node, valid tubes and tip leaves.
* **sharp angle (≈ π)** — parallel-transport frames do not flip; children
  remain welded to the parent centerline.
* **many small leaves** — leaf/vertex/triangle budgets clamp the instances.
* **extreme parameters** — hard caps cannot be escaped; tiny budgets
  terminate with explicit prune records.
* hierarchy/export round trips, OBJ group coverage, invalid-parameter
  rejection, and the no-occlusion-deletion guarantee.
* **rule migration** (`tests/rule_migration.rs`, fixtures under
  `tests/fixtures/`): implicit-unit resolution and the cm header, seed/level/
  material preservation, unknown rules reported as pending rather than
  defaulted, byte-identical geometry across both rule paths, a same-seed
  cross-algorithm divergence case, layer identity + world-extent evidence,
  attachment weld evidence, and explicit-syntax rejection of bare numbers.
* **GUI engine** (`plant-gui`): headless rule apply, v1->v2 migration,
  error feedback that preserves the current plant, and pending-rule counting.

## Determinism notes

All math is plain `f32` with a fixed operation order; the crate does not read
the floating-point environment, time, or any ambient state. The JSON writer
emits floats via a shortest round-tripping representation and keeps seeds as
unsigned 64-bit integers so they survive archives without truncation.
