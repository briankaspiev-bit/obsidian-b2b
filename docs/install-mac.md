# Installing Obsidian B2B on a Mac

These steps are for anyone testing Obsidian B2B on a Mac. No coding needed. It runs on
Apple-chip (M1 and newer) and Intel Macs with macOS 11 Big Sur or newer.

## 1. Download

Download **Obsidian-mac.dmg** from:

**https://github.com/briankaspiev-bit/obsidian-b2b/releases/tag/mac-build**

No GitHub account is needed. The page always has the newest test build.

## 2. Install

1. Double-click **Obsidian-mac.dmg** in your Downloads.
2. Drag **Obsidian B2B** onto the **Applications** folder in the window that opens.
3. Eject the disk (the eject arrow next to it in Finder's sidebar).

It is called **Obsidian B2B**, so it won't replace the Obsidian notes app if you have that.

## 3. Open it the first time

The test build isn't signed by Apple yet (that needs a paid developer account, which
comes before a public release), so macOS asks you to confirm once.

1. Open **Applications** and double-click **Obsidian B2B**.
2. macOS says it can't verify the app. Click **Done** (not Move to Trash).
3. Open **System Settings** → **Privacy & Security**, scroll down to **Security**, and
   click **Open Anyway** next to "Obsidian B2B was blocked". Enter your Mac password,
   then click **Open Anyway** again.

On macOS 14 Sonoma or older you can instead **right-click** (or Control-click) the app,
choose **Open**, then **Open** again.

After this the app opens normally with a double-click.

**If macOS says the app "is damaged and can't be opened"**: open **Terminal** (in
Applications → Utilities), paste this line, press **Enter**, then open the app again:

```
xattr -dr com.apple.quarantine "/Applications/Obsidian B2B.app"
```

## 4. Allow the microphone and network

- **"Obsidian B2B would like to access the microphone"**: click **Allow**. This is how
  it hears your mixer or audio interface. If you clicked Don't Allow by mistake, open
  **System Settings** → **Privacy & Security** → **Microphone** and turn on
  **Obsidian B2B**, then quit and reopen the app.
- **"Accept incoming network connections?"** or **"find devices on your local
  network?"**: click **Allow**. Without this the other DJ can't hear you.

## 5. Your audio

In **Booth Check**, pick your audio interface or mixer as the input, and your
headphones as the output. Capturing the laptop's own sound (instead of a mixer) only
works on Windows for now.

**No mixer?** Pick **Test music: Groove** (or **Test music: Bells**) as the input. The
app plays its own built-in music to the other DJ, so you can test without any gear.

## Updating

Download the new dmg from the same page, drag it into Applications again, and choose
**Replace**. You may need to do step 3 once more.
