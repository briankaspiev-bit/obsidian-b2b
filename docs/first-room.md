# Your first room, no gear needed

Two DJs, two laptops (Windows or Mac), home Wi-Fi is fine. Each of you uses the
app's built-in music and mixes it on the built-in deck.

## Get in the room

1. One of you clicks **CREATE ROOM** and sends the other the code. The other
   types it under **Join a booth** and clicks **JOIN ROOM**.
2. In **Booth Check**:
   - **What you send**: one of you picks **Test music: Groove**, the other
     **Test music: Bells** (so you can tell the two songs apart).
   - **Where you listen**: your headphones. On many Windows laptops the
     headphone jack is called **"Realtek HD Audio 2nd output"**.
   - Click **RUN BOOTH CHECK**. Yellow lines are warnings (Wi-Fi, for example); once
     **Connection** and **Booth Sync** are green you can go live.
3. Both press **I'M READY**. The app remembers your picks for next time.

## Mixing

Whoever created the room starts on air, already playing. In your headphones
you hear your own deck and the other DJ together.

| Key | Does |
| --- | --- |
| **P** | Play / pause your deck |
| **S** | Sync your deck to the other DJ's beat |
| **↑ ↓** | Your fader: your volume, for you **and** the other DJ |
| **← →** | While you're on air: blend the other DJ's song in or out. When they take over, the blend passes to them: their song keeps playing at that level and they bring their fader up. |
| **Space** | Take over (you go on air) |
| **R** | Ready: tells the other DJ you're about to take over |

To trade off: the DJ who is cueing presses **P**, then **S**, brings their
fader up, presses **R**, then **Space**.

Keep your fader up while you're on air: pulling it down silences you for the
other DJ too. If the set goes quiet, the orange line under the beat meter says
why.

## No partner around? Play with the Robot DJ

The Robot DJ joins your room from a server in the cloud, plays **Bells**, and
takes over every so often so you can practise handoffs against a real
internet connection. Afterwards it reports how your sound reached it.

1. Click **CREATE ROOM** and send the code to Claude in the project chat.
2. Stay in Booth Check. After a few minutes **Robot DJ** joins: pick your
   music and headphones, run the check, press **I'M READY**.
3. Mix as usual. About a minute after you go on air the robot syncs, shows
   READY and takes over; press **Space** to take it back.

(Developers: Actions → robot-dj → Run workflow, with the room code.)
