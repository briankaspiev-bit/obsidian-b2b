# obsidian-live: try a remote B2B on Windows

`obsidian-live.exe` is one DJ's side of a remote back-to-back set. It runs the real
booth engine (Opus over UDP, fixed-delay playout, loss recovery, beat-aligned
monitoring) on the laptop's sound card. It is a plain console app for now; the
desktop app's Live Session screen will drive the same engine later.

You need: Windows 10 (version 2004 or newer) or 11, headphones, and for the two-person
test, both laptops on Ethernet.

## Get it running

1. Put `obsidian-live.exe` in a folder, e.g. `C:\obsidian`.
2. Open that folder in File Explorer, click the address bar, type `powershell`, press Enter.
3. Windows may warn that the app is unrecognised (it is not code-signed yet):
   **More info → Run anyway**. When it asks about the firewall, allow **private and public** networks.
4. Run `.\obsidian-live.exe devices` to see your sound cards.
5. Can't hear anything? Run `.\obsidian-live.exe tone` (or `tone --output "part of a name"`) for a test beep. The session screen also shows a "sound card" line: if its callbacks count up and the level meter moves but you hear nothing, the sound is going to a different jack or Windows has the app muted (Settings → Sound → Volume mixer).

## Practise alone (no partner, no DJ gear)

```
.\obsidian-live.exe solo
.\obsidian-live.exe solo --music "C:\Users\you\Music\track.mp3"
```

A ghost DJ plays through the real engine over a simulated New York ↔ London link
(about 75 ms each way, with that route's jitter and packet loss). It starts on air.
Your deck holds your track (or a built-in test groove), cued to its first beat.

What to try:

1. Listen for about 10 s while the link is measured; then you hear the ghost.
2. Press **P** to start your track, then **S** (SYNC): your deck matches the ghost's
   tempo and lands on its beat as you hear it, and guesses the bar so claps line up. If the bars still feel off, **[** / **]** move your track one beat. Or beatmatch by hand: **-** / **=** change
   the speed 0.1% at a time, **,** / **.** push the beat 10 ms back or forward.
3. Bring yourself in with **↑** (your fader), then press **SPACE**: you're on air.
4. The ghost keeps playing under you for 15 s, synced to you, then fades out over 15 s.
5. A minute later (or when you press **G**) the ghost cues its next track, syncs to
   you as it hears you, fades in, and takes the air back. Fade yourself out with **↓**.

While you're on air, the ghost reaches you about two network trips late; the app adds
just enough extra delay that it lands on your beat ("+N ms to land on your beat").

Other paths: `--path same-city`, `--path nyc-tyo` (Tokyo, ~140 ms), `--path bad-wifi`
(what a bad home Wi-Fi does). `--ghost-music a.mp3 --ghost-music b.mp3` gives the ghost
your own tracks. `--system` sends whatever Windows is playing instead of the deck.

## Two laptops over the internet

Until connect-by-code lands, one side hosts and the other joins by IP address.

**Host** (the one whose router you can configure):

```
.\obsidian-live.exe host --name Brian --music "C:\Music\track.mp3"
```

It listens on UDP port 9000. Find your public IP (search "what is my IP"). If your
partner can't reach you, forward **UDP 9000** on your router to this laptop.

**Join**:

```
.\obsidian-live.exe join --peer 203.0.113.7:9000 --name Dana --system
```

`--system` sends whatever the laptop is playing (rekordbox, Serato, Spotify...), except
this app's own sound, so your partner never echoes back. Play your music as usual.
Not yet verified: whether `--system` picks up DJ software that plays straight to a
controller's own sound card. If your partner hears nothing from you, route the mixer's
record out into an audio interface and use `--input "<its name>"` (names come from
`devices`). To hear your partner in a controller's headphone jack, try
`--output "<controller name>" --output-channel 3` (channels 3/4).

Both sides see the same screen: who is on air, how late the partner reaches you, the
safety buffer, rescued and patched-over packets, your fader and the partner's volume.

## Keys

| Key | Does |
|---|---|
| SPACE (or T) | Take over: you're on air |
| R | Ready: tell your partner you're cued to take over |
| ↑ / ↓ | Your fader (what your partner and the room get) |
| ← / → | Partner's volume in your headphones |
| P | Play / pause your deck |
| S | SYNC on/off (follows whoever is on air while you're coming in) |
| C | Cue: back to the first beat |
| [ / ] | Jump one beat back / forward (line up the bars) |
| , / . | Nudge the beat 10 ms back / forward |
| - / = | Speed −/+ 0.1% |
| G | Solo: bring the ghost back now |
| Q | Quit |

## Afterwards

Each session saves `recordings\session-<time>\`: `sent.wav` (you, as sent),
`monitor.wav` (your partner, exactly as you heard them, sample-aligned with sent.wav)
and `report.json`. Send both sides' folders to build the combined master with
`tools/merge`.

## Known limits

* Wi-Fi: packet bursts on bad Wi-Fi are still audible; use Ethernet to perform.
* No encryption yet; fine for two friends, not for strangers.
* Joining by IP needs a reachable host; the room-code service will remove that step.
