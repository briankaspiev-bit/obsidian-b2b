# Installing Obsidian B2B on Windows

These steps are for anyone testing Obsidian B2B on a Windows 10 or 11 PC. No coding needed.

## 1. Download

Open the download page:

**https://github.com/briankaspiev-bit/obsidian-b2b/releases/tag/windows-build**

It always has the newest test build. No GitHub account is needed. You will see one or both of these files:

- **Obsidian-B2B-installer-windows** (`.exe` or `.msi`): the desktop app. Use this once it appears.
- **Obsidian-B2B-test-tools-windows.zip**: the engine test programs used for the first connection tests.

## 2. Unblock the zip before opening it

Windows marks downloaded files as "from the internet". Clearing that once on the zip saves a warning for every program inside it.

1. Open your **Downloads** folder.
2. Right-click **Obsidian-B2B-test-tools-windows.zip** and choose **Properties**.
3. At the bottom of the **General** tab, tick **Unblock**, then click **OK**. (If there is no Unblock box, skip this.)
4. Right-click the zip again, choose **Extract All...**, then **Extract**.

## 3. The "Windows protected your PC" warning

The test builds are not signed yet (signing costs money and comes before a public release), so Windows does not recognize the publisher. When you open the installer or a program, you may see a blue box saying **"Windows protected your PC"**.

1. Click **More info**.
2. Click **Run anyway**.

This only happens the first time for each file.

Your browser may also warn that the file "isn't commonly downloaded". In Edge, click the **...** next to the download, then **Keep**, then **Show more**, then **Keep anyway**. In Chrome, click **Keep**.

**If Windows says the app was blocked and there is no "Run anyway" button**, your PC has **Smart App Control** turned on, which blocks all unsigned apps. Don't turn it off to get around this; tell Brian, because it means we need to sign the build.

## 4. Allow it through the firewall

The first time Obsidian talks over the network, Windows Firewall asks whether to allow it. Tick **Private networks** and click **Allow access**. Without this, the other DJ can't hear you.

## 5. Quick check that it works on your PC

For the test programs (the zip):

1. Open the extracted **Obsidian-B2B-test-tools-windows** folder.
2. Click the address bar at the top of the window, type `cmd`, and press **Enter**. A black window opens in that folder.
3. Copy this line, paste it into the black window (right-click pastes), and press **Enter**:

   ```
   obsidian-bench.exe run --only clean --duration 30 --profiles profiles.json
   ```

4. Wait about 30 seconds. A results table appears and a **bench-out** folder is created. Open `bench-out\summary.md` with Notepad and send it to Brian.

For the desktop app (the installer): double-click it, follow the steps, then open **Obsidian B2B** from the Start menu.

## Updating

Download again from the same page and repeat. Each new build replaces the old one on that page.

## For developers

The builds come from `.github/workflows/windows.yml`. It runs on every pull request and every push to `main` on a `windows-latest` machine:

- tests and builds every Rust workspace in the repo (the engine at the root, standalone tools such as `tools/merge`, and `services/` when it lands), then runs a 30-second loopback session between two engine peers;
- tests and builds the desktop UI, and builds the Tauri installer once `apps/desktop` has a `tauri.conf.json`;
- on `main`, replaces the `windows-build` pre-release with the new files.

Every pull request's run also has the files under **Artifacts** on its Actions page (GitHub sign-in needed).
