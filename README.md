# Auto BOM (standalone)

Import a STEP file, read the assembly, and build a bill of materials. It is the BladeDocs website's Auto BOM tool as a
desktop program. Nothing is uploaded; the STEP file is read as text (no geometry is loaded).

## Build (Windows, macOS, Linux)
Install Rust from https://rustup.rs, then in this folder:

    cargo build --release

Programs appear in `target/release/`: `autobom.exe` (window) and `autobom-cli.exe` (command line).
Linux also needs GTK3 development files (`libgtk-3-dev`). The GitHub Action in `.github/workflows/build.yml` builds all three systems.

## Window
File > Open STEP file (or drop one on the window). **Review** walks through each proposed entry: edit the name, quantity,
list, cost (`.62` becomes `$0.62`), up to 5 links, description and note, untick components that are a different part,
then Confirm, Skip or Ignore forever. Look-alike names (Post L / Post R, Screw-1 / Screw-2) ask "same part?" and the answer is remembered.
**Bill of Materials** is the editable table with totals; **Parts** lists every name found. Export CSV (same layout as the
website) or JSON from the File menu. Save the project to keep costs, links and answers: opening the same model again later
shows only what changed.

## Command line
    autobom-cli model.step --csv bom.csv --json bom.json --save job.autobom.json
    autobom-cli model.step --project job.autobom.json --csv bom.csv
Options: `--import bom.csv`, `--merge-similar`, `--keep-similar`, `--parts`.

## How parts are found
Products and `NEXT_ASSEMBLY_USAGE_OCCURRENCE` placements are followed, quantities multiply down sub-assemblies, and bodies
named `SOLID` take their component's name. `(Mirror)` and `<2>` copies count with the original.

## Limits
No 3D view, so you cannot click a part, and mirrors are detected by name only. Components with no geometry are left out.

## Viewport
The right-hand panel draws the selected part as an outline (edges, circles, splines) that you can orbit (drag), pan
(right-drag) and zoom (scroll). X, Y and Z tape measures in millimetres sit beside it, with round tick marks and the
overall size. Inch files are converted to mm. It shows outlines, not shaded surfaces, and parts stored only as meshes show no outline.
