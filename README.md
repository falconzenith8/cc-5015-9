# Procedural Virtual Plant Studio

A deterministic procedural plant generator written in Rust. Given a parameter
set it grows a trunk, recursively places branches on the parent branches'
curved centerlines, and instances leaf blades — all from real tube/leaf
geometry, never a pre-made model or particle billboard.

## Workspace layout

| Crate | Role | Third-party deps |
| --- | --- | --- |
| `plant-core` | Deterministic generation, math, geometry, validation, export | none |
| `plant-cli` | Headless generation / validation / stress / export | `plant-core` |
| `plant-gui` | Realtime `egui`/`eframe` parameter editor with orbit viewport | `eframe`, `egui` |

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
```

### GUI

```bash
cargo run --release -p plant-gui
```

Left panel: seed, branch count, angle, jitter, ratios, bend, taper, leaf
parameters and all resource budgets, with live rebuild (or manual via the
"Rebuild on change" toggle). Right panel: orbit-drag / scroll-zoom viewport
drawing the actual generated splines and leaf instances. A native display
(Wayland/X11) is required; on a headless host the binary exits with a clear
window-system error.

## Test matrix

`cargo test` runs 26 tests, including the required scenarios:

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

## Determinism notes

All math is plain `f32` with a fixed operation order; the crate does not read
the floating-point environment, time, or any ambient state. The JSON writer
emits floats via a shortest round-tripping representation and keeps seeds as
unsigned 64-bit integers so they survive archives without truncation.
