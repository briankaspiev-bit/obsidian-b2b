Obsidian B2B - Windows test build
==================================

This is an early test build. It is not signed yet, so Windows may say it
"protected your PC". Click "More info", then "Run anyway".

What is in this folder (some may not be here yet, depending on what has shipped)
  obsidian-peer.exe    One side of a remote B2B session (the booth engine).
  obsidian-bench.exe   Runs a short practice session on this computer and scores it.
  obsidian-netem.exe   Simulates a bad internet connection (used by the bench).
  obsidian-merge.exe   Builds the final master mix from both DJs' recordings.
  profiles.json        Network conditions the bench can simulate.

Quick check that the engine works on this computer
  1. Open this folder in File Explorer.
  2. Click the address bar, type  cmd  and press Enter. A black window opens.
  3. Type this line and press Enter:
       obsidian-bench.exe run --only clean --duration 30 --profiles profiles.json
  4. If Windows Firewall asks, tick "Private networks" and click "Allow access".
  5. After about 30 seconds a table appears. The results are saved in the
     bench-out folder (open bench-out\summary.md in Notepad).

Full steps, including every Windows warning you may see: docs/install-windows.md in the
Obsidian B2B repository on GitHub.
