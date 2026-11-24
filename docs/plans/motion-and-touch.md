# Motion sensors and the touchpad

## Goal

A PlayStation pad has three kinds of input beyond its buttons, sticks and triggers: a touchpad that reports where fingers are, a gyroscope that reports how fast the pad is turning, and an accelerometer that reports which way is down and how the pad is shaken. The kernel gives each its own node beside the pad's. CtrlAssist reads none of them. The touchpad's click is a button and is routed as one; a virtual DualSense has a touchpad and sensors that report nothing.

That was a choice of scope: the routing model is built for controls that rest at zero. It has become a loss. A hidden pad now has every one of its nodes hidden, so that a game sees one controller and not a second, partial one; with that, a game can no longer read the real pad's sensors or touchpad for itself, and nothing carries them to the virtual pad. While a setup hides a PlayStation pad, gyro aiming, tilt and touchpad gestures are gone.

The goal is for a virtual DualSense to carry them, by rules that keep what each mode promises: the primary player stays in charge, and what the assist does is bounded in time and in scope.

## State

Not started. The research is in and the first decisions are made (Decided after the research); the rest are open. A spike comes once the review's fixes have settled, and the build after 0.5.0.

## What the modes promise

From the 0.4.0 README, which is where the modes were first described, and which the rewrite keeps:

- **Priority.** The assist overrides a control while it is active on it, past a deadzone, and the control returns to the primary when the assist lets go. "Partial and asynchronous assistance."
- **Average.** The two blend while both are active, and otherwise whoever is active is heard. "Cooperative input and subtle corrections."
- **Toggle.** Everything comes from the pad that has control, and a button hands over. "Fine-grain conflict-free control."

The maintainer's account of what an assist is for: the helping player supplements the primary player's fine motor skills, and some modes respect the primary player's autonomy by keeping interventions temporal and scoped. A rule for motion or touch is right if it reads as the same promise.

The same README says more that bears on it. Who it is for: "partial asynchronous input, i.e. offloading camera angle management, movements requiring speed and precision, or on standby backup during difficult combat encounters". Managing the camera is the first thing named, and where a game aims by gyro the camera is the gyro: an assist who takes the camera over wants the turning, which the first position below allows in priority and toggle. Why a pad isn't simply passed back and forth: handing over breaks immersion, adds friction, and costs the primary player agency ("waiting for returned control") and learning ("no haptic feedback hinders recognizing cues"). So the primary player goes on holding their own pad and feeling it throughout, and whatever is done with motion must not ask them to put it down. And what 0.4.0 did not do: "tactile triggers, gyroscopic and accelerometer motion tracking, or more exotic force feedback waveforms are not yet supported". Motion was never carried; whether a game could still read a hidden pad's sensors for itself under 0.4.0's ways of hiding has not been checked.

Its Background names what the feature is modeled on, which the research is to read for what each does with motion and touch: Controller Assist (Copilot) on Xbox and Windows, Second Controller Assistance on PlayStation, and InputPlumber, an input router for Linux that makes one virtual pad of several devices, a handheld's separate motion sensor and touchpads among them.

## Decisions (first position)

**The touchpad follows the mode's own rule.** A touchpad with no finger on it is at rest, so it is a control like a stick: in priority the assist's touch overrides while a finger is down, in average the nearest thing to a blend is to be decided (two fingers' positions don't average into a third that means anything), in toggle it is the active pad's. Its click is already a button.

**The gyroscope's rate follows the mode's own rule too.** A pad held still, or lying on a lap or a table, turns at a rate near zero, so a rate is a control that rests at zero, with a deadzone as a stick has: in priority the assist's turning overrides while it exceeds the deadzone, in average the two rates blend while both do, in toggle it is the active pad's. The deadzone wants a default that clears a hand's tremor, and to be a profile's value as a stick's is.

**The accelerometer does not rest, and is not blended.** It reports gravity, which is never zero, and two pads' gravity averaged is a shorter vector pointing nowhere real. In toggle it is the active pad's. Outside toggle it is undecided; the first position is the primary's, always: the primary's pad stays the truth of which way is down, the assist helps through turning alone, and where a game estimates the pad's pose from both sensors, its own correction for drift brings the estimate back to the primary's pose after the assist lets go. The alternatives are under Open questions.

**One source pad has nothing to decide.** In unicast and multicast there is one pad, and its touchpad and sensors go where its other controls go.

**Only a virtual DualSense can carry them.** The other identities have no touchpad and no sensors to report through.

## Open questions

Each is to be answered before the decision it bears on is settled.

- **How games use the gyroscope and the accelerometer.** What is done with a rate (aiming, where turning the pad turns the view; flick stick; a cursor), what is done with gravity (steering by tilt, a level, shake), and what is done with both together (a pose estimated by sensor fusion). The mathematics of each, and so what each does when the two sensors disagree for a moment, when gravity jumps, and when a rate is the sum or the blend of two hands'. Research is under way.
- **What the platform does between the pad and the game.** What the kernel's driver, SDL and Steam Input each do with a DualSense's sensors and touchpad: units, rates, calibration, what a game is handed (raw readings or a pose), and what Steam's own gyro mappings read. Whether a game can tell a virtual DualSense's sensors from a real one's.
- **What others have done.** Whether any tool that lets two controllers act as one (the consoles' own assist and copilot features, Steam, remappers) carries motion or touch, and by what rule.
- **The accelerometer outside toggle.** The primary's always, as above. Or the pad's that the rate comes from at each moment, which is always self-consistent and makes gravity jump whenever the assist moves. Or made up: the virtual pad as a rigid body that starts at the primary's pose and is turned by the merged rate, which is smooth and self-consistent and drifts from where the primary's pad really is. The research decides which of these a game could tell apart, and what each costs.
- **Whether an assist should be able to help with steering by tilt outside toggle.** Not known. If so, tilt (the angle gravity makes with level, which does rest near zero) could be a control of its own that the modes blend, apart from the raw readings.
- **Whether presets should say more.** The maintainer's thought: widen what a profile can express so that it can choose how motion is mapped, where today a preset is a mode and a handful of values. What the choices would be, and whether they are a key or two on the presets or a new kind of node in a profile's graph.
- **The desktop's use of the touchpad.** Outside a game, a DualSense's touchpad moves the desktop's pointer, and Steam can map it. While a setup hides the pad its touchpad's nodes are hidden with it, and the pointer follows the touchpad all the same: seen on Plasma with a DualSense on USB and a DualShock 4 on Bluetooth, the compositor having its descriptors from logind, which the nodes' group doesn't stop. Stopping it would take the rule telling libinput to ignore a hidden pad's touchpad. A pad that is grabbed and not hidden keeps its touchpad, a grab being of the pad's own node alone. Whether a setup should leave the desktop its pointer is a question of its own.
- **Cost on the input thread.** Two more nodes for each PlayStation pad, the sensors' at a high rate. To be measured with the bench before it is built on.

## What the research found

Three surveys were made on 2026-11-23, by agents reading public sources. What a source says is given with the source; what is reasoned from it is marked as inference. None of it has been tried here on a pad.

### The gyroscope, and aiming by it

**A game integrates a rate.** Aiming by gyro is `camera.yaw += rate.yaw × sensitivity × dt`, and the same for pitch: a relative device, as a mouse is ([GyroWiki, "The Gyro is a Mouse"](http://gyrowiki.jibbsmart.com/blog:good-gyro-controls-part-1:the-gyro-is-a-mouse)). The camera's angle is continuous whatever the rate's source, and no absolute pose of the pad maps to where the camera points. Sensitivity is in real turning to turning in the game: at 1, 37.5° of the pad is 37.5° of the camera. Steam's guidance to developers is to add up every source of camera turning before moving the camera, and not to take the last one active ([Steam Deck recommendations](https://partner.steamgames.com/doc/steamdeck/recommendations)).

**Three spaces, two of which need gravity.** Local space uses the pad's own axes and no accelerometer. World space and player space both take the estimated gravity vector to tell a turn about the vertical from one about the pad's own axis; player space forgives an error in it of about 45° (a relax factor of 1.41; JoyShockMapper uses 2, about 60°), and world space shows any error as a cosine and as leakage between axes ([GyroWiki, "Player Space Gyro"](http://gyrowiki.jibbsmart.com/blog:player-space-gyro-and-alternatives-explained)). Player space is the default the library most games would copy recommends ([GamepadMotionHelpers](https://github.com/JibbSmart/GamepadMotionHelpers)).

**Gravity is estimated, not read.** The estimate is turned by the gyro and pulled toward the accelerometer, at a rate of about 1 a second when the pad is still, 0.1 when shaken, and capped near a tenth of the turning rate ([GyroWiki, "Finding Gravity with Sensor Fusion"](http://gyrowiki.jibbsmart.com/blog:finding-gravity-with-sensor-fusion)). "The accelerometer alone cannot reliably tell you the direction of gravity."

**Small rates are already treated specially.** Smoothing below a threshold and "tightening", a soft scaling down of rates under about 5°/s, are the community's answer to a hand's shake; a hard cutoff is advised against, and the way to say "I am not aiming now" is a button that turns the gyro off or on while held (JoyShockMapper's `GYRO_ON`/`GYRO_OFF`). An acceleration curve makes sensitivity grow with speed, so it is not linear in the rate.

**Bias is the game's to remove, and it assumes one pad.** The kernel's driver applies the pad's calibration for scale and subtracts no bias (a change of 2023), so whoever reads the sensor node calibrates drift itself. GamepadMotionHelpers does it two ways: when gyro and accelerometer are both still for a time, the mean is the bias; and by comparing the gyro with the turning the accelerometer's gravity implies, taking the difference for bias when the turning is steady. An uncorrected bias of 0.5°/s at a sensitivity of 2 is 60° of drift a minute.

**What a DualSense gives.** Through the kernel, a second input device, "Motion Sensors", with the accelerometer on `ABS_X/Y/Z` at 8192 to a g, the gyro on `ABS_RX/RY/RZ` at 1024 to a degree a second, and a timestamp in microseconds as `MSC_TIMESTAMP`. SDL pairs it with the pad by its `uniq` string and hands a game the rate in radians a second and acceleration in m/s² with gravity in it: readings, and no pose. Steam's own API adds a fused quaternion. Reports come at 250 Hz on USB and, by SDL's comments, 1000 Hz on Bluetooth.

**Aim assist goes when gyro is on.** Fortnite and The Finals say so of themselves. One player on gyro and the other on the stick loses the stick its aim assist in such games.

**Not found.** Measured noise or bias for these pads' sensors; a healthy hand's tremor in degrees a second from a primary source (reasoned to be 2 to 6°/s at its peaks, at 8 to 12 Hz, which is the band fine aiming is in); any game saying whether it reads rates or a platform's pose.

**What follows for two pads (inference from the above).**

- **Timestamps come first.** A virtual pad must make its own clock. Passing on each source pad's timestamps makes the clock jump on a change of source, and SDL turns a jump backward into a very long step, which every integrator downstream then takes.
- **Bias must be removed here, for each pad, before anything is mixed.** A game that has learned one pad's bias drifts on the other's; a sum carries both biases; and the game's own calibrator, fed two pads, either refuses to calibrate or learns a mixture. Each pad's stillness is detectable from its own two sensors.
- **A rate can't tell a held pad from an aiming one.** A pad lying on a table (under 0.3°/s, its accelerometer constant) is cleanly told from a held one. A held and idle hand is not told from one aiming finely: both are a few degrees a second. So the deadzone that makes priority work for a stick does not carry over to the gyro as it stands: a threshold high enough to pass over tremor drops fine aim, the lost start of each movement makes a turn come up short, and near the threshold the two pads trade places at the tremor's own rate.
- **A hard switch between pads is otherwise benign** in local space, like handing over a mouse: the camera stays where it was left.
- **A sum is exact for a rate** and lets either aim, and works well when the idle pad lies flat; held, the idle hand's tremor goes straight to the camera. The two pads' axes agree only if they are held alike.
- **An average halves the sensitivity** of whoever moves alone, which breaks the learned measure of a turn, and blended only while both move it changes gain by half at each crossing.
- **Gyro from one pad and accelerometer from another is the worst of the mixtures.** Local space doesn't notice. Player space is unaffected while the two pads' poses differ by less than about 45°, weakens past that and reverses past 90°. The gravity estimate is turned by one pad and pulled toward the other. And calibration by fusion takes one player's steady aiming, unseen by the other pad's accelerometer, for bias, and corrects it away. This weighs against the first position's "the primary's accelerometer always".
- **JoyShockMapper's way with two Joy-Cons** is to let each move the mouse by itself, each after its own calibration and curve: a sum of results, not of signals.

### The accelerometer: tilt, pose and shakes

**Steering by tilt is done three ways, and each takes a mixed signal differently.**

- *From gravity alone.* The angle is `roll = atan2(a.x, a.y)` of the accelerometer's reading, sometimes through a low-pass of about a second ([Unity's sample](https://docs.unity3d.com/2019.4/Documentation/Manual/MobileInput.html)). It has no memory: nothing to drift, nothing to recover. The Wii's games before its gyro could do nothing else, and a Dolphin bug shows them reading the raw vector, linear acceleration and all ([Dolphin 11776](https://bugs.dolphin-emu.org/issues/11776)).
- *From a pose the platform fuses.* The Switch and the PlayStation hand a game an orientation beside the readings; Steam's API hands a quaternion. Emulators that stand in for those platforms fuse it themselves, and their code is where the constants can be read.
- *From the gyro integrated.* Steam Input's own "gyro to joystick deflection" does this, and users report it losing its center, as an angle with nothing to anchor it would.

**How a fused pose recovers** after its gyro was fed turning its accelerometer never saw: a Mahony filter (Cemu, and with other gains the Switch emulators) pulls back at `Kp × sin(error)`, a time constant of 2 to 3.3 s; Madgwick's slews at a constant 11.5°/s; Dolphin's in about a quarter second; GamepadMotionHelpers closes a large error fast and the last 3° slowly. In all of them only pitch and roll come back, smoothly and with no jump; an error in heading is never corrected. One library ignores an accelerometer that disagrees by more than 10° for about 5 s before it recovers at all ([x-io Fusion](https://github.com/xioTechnologies/Fusion)).

**Some consumers check that gravity is 1 g long.** The Switch emulators' filter corrects drift only between 0.75 and 1.25 g, and takes a pad for still, to learn its bias, only between 0.9 and 1.1 g. GamepadMotionHelpers uses the direction alone and counts a change of length as shaking.

**Shakes are found in what is not gravity:** a magnitude over about 1.3 g for a quarter second ([Seismic](https://github.com/square/seismic)), or acceleration with gravity taken out over a threshold. A vector that is always exactly 1 g can never shake.

**Not found.** Which of the three ways any named racing game steers by; Sony's and Nintendo's own documents (what is above is from homebrew headers and emulators); how Steam fuses its quaternion; any report of a game misbehaving when gravity jumped. The consequences below rest on libraries' and emulators' code, not on games seen to do it.

**What follows for two pads (inference).**

- **The primary's gravity with the other's rate** leaves a gravity-only game steering by the primary alone, as if the assist weren't there; makes a fused pose follow the assist and then drift back over seconds, with the heading left off for good; and has a calibrator by fusion learn the assist's steady turning as bias.
- **Both switched together** steps a gravity-only game's steering to the other pad's tilt, which is what was asked; a fused pose settles for seconds each way; and a calibrator by fusion can take the swing for a bias of tens of degrees a second, a spin of the camera. The most damaging case where that calibrator is on.
- **Made up, as a rigid body turned by the merged rate,** is perfectly consistent and no filter can notice it. But nothing ties it to the real down: the gyros' bias walks the steering's center away, and whatever the assist turned stays turned after they let go, so the primary must hold their pad askew to drive straight. And it can't shake. Tying it back to the pads' measured gravity with a gain makes it a complementary filter, and then the assist's part fades in a time of one over the gain: a nudge, not a held angle.
- **The two vectors averaged** points along the bisector, which for a roll in one plane is exactly the mean of the two roll angles: a gravity-only game gets a true blend of the two steerings, the best of the options for such a game. Its length is `cos(half the angle between)` of a g: 0.97 at 30° apart, 0.90 at 52°, 0.75 at 83°, nothing at opposite. So it wants making 1 g long again, and guarding where the two pads are nearly opposed.
- **Blending tilt angles and making a gravity vector of the result** is sound, and for pads near level it is the same as blending the unit vectors, which has no singularity at a pad held on end and no wrap at 180°, so the vectors are the better thing to blend. What it loses: linear acceleration, unless carried apart; exact agreement with the gyro, which holds only near level; and it mixes two people's resting poses, since nobody holds a pad level. Blending each one's departure from their own rest, and adding the result to the active player's rest, avoids the last.
- **The signal a game is least likely to notice,** by this survey: gravity's direction as a weighted mean of the pads' directions, made 1 g long; linear acceleration carried apart if shakes matter; and any change of weights ramped, a step in them being a jump of gravity in small.

### What others do

**Nobody blends two motion sensors.** Every system found picks one, or shows both side by side for the game to choose.

- **PlayStation's assist controller.** Sony's accessibility page: "Some of the DualSense features, such as motion sensor, haptic feedback and adaptive triggers, will be automatically turned off when the controller is being used as an assist controller." So motion is the main controller's, the primary player's, alone. The support page lists haptics, adaptive triggers, the microphone and custom buttons as not available on the assist controller, and says nothing of the touch pad, the speaker, the light bar, or how two inputs at once combine ([support](https://www.playstation.com/en-us/support/hardware/second-controller-assistance/), [accessibility](https://www.playstation.com/en-us/accessibility/)). An Access controller, which has neither sensor nor touch pad, joined with a DualSense takes its "motion sensor, and touch pad swipe" from the DualSense.
- **Xbox's Controller Assist** links two controllers "as if they were a single controller"; its pads have no motion sensor or touchpad. How analog values combine isn't in what could be read of Microsoft's page; a paper of 2026 describes it as summed, two opposite steerings going straight.
- **Two Joy-Cons as one pad**, the nearest thing to two sensors merged: Nintendo's system gives a game both, apart; games mostly ignore the left; SDL and Steam take the right and SDL also offers each by name; JoyShockMapper defaults to ignoring the left and offers ignore-left, ignore-right, ignore-both and use-both.
- **Tools that group controllers.** reWASD gives its virtual gyro to the first controller of a group. Cemu takes motion from the first attached controller marked to use it, binds buttons controller by controller, and rumbles them all. Emulators generally let the device that gives motion be chosen apart from the one that gives buttons.
- **InputPlumber** makes a composite of several source devices, each with a group (gamepad, IMU, touchscreen and so on) and its own included and excluded events, a source that is blocked, or one kept for output only. Motion has a place of origin in its vocabulary, `Gyroscope:Left`, `:Right` or `:Center`, and touchpads likewise. Its virtual DualSense carries one stream of motion and one touchpad of two contacts, and advances the sensor timestamp itself because SDL takes differences of it.

**How people share a controller.** Interviews with fourteen people who do ([Ahmetovic et al.](https://arxiv.org/html/2509.02132v1)): keeping each one's controls strictly apart is more usual than overlapping them; the camera is the control most often handed to the helper; roles are mostly agreed before play, and some hand over everything for a hard part; and not knowing who has which control "can lead to confusion and frustration". A study of shared control ([Mascetti et al.](https://arxiv.org/html/2601.11218v1)) tells combining two commands from selecting one of them, and asks that a system show, as it happens, what the helper is doing.

**Tremor, measured.** Essential tremor at the hand, by gyroscope: 0.5°/s RMS where mild, 15.2°/s where severe, at about 6 Hz ([Technology and Health Care, 2020](https://pmc.ncbi.nlm.nih.gov/articles/PMC7369080)). Physiological tremor is at 6 to 12 Hz, a Parkinsonian rest tremor at 4 to 6. No tool was found that offers a filter as one for tremor; what there is are a speed below which output stops, a speed below which it is reduced or smoothed, and the 1€ filter, a low-pass whose cutoff rises with speed, which Steam and DS4Windows both use.

**A vocabulary of settings**, as it recurs across Steam Input, JoyShockMapper, DS4Windows, reWASD, InputPlumber and the emulators:

- *Source.* Which device gives motion; which of several sensors (left, right, both, neither); the first that can; motion off on a second device; which touchpad, and several onto one; a source blocked or kept for output alone.
- *Engagement.* Always on; on while a button is held, off while held, toggled; by a button, a touch of a stick or a pad, a grip; released to move the hands back ("ratcheting"); a command to recenter.
- *Space and axes.* Yaw, roll or a blend of them for the horizontal; local, player or world space; inversion; a scale for each axis; a region of a touchpad.
- *Filtering.* A speed below which nothing passes; a speed below which it is reduced or smoothed; the 1€ filter; calibration when still.
- *Conversion.* Passed through as it is, to a virtual pad that has sensors; or turned into a mouse, a stick's deflection by rate (a camera) or by angle (a tilt), buttons or swipes.

**Not found.** Microsoft's own account of how two controllers' values combine; Sony's on the touch pad, speaker and light bar under assist; how JoyShockMapper's use-both combines; anything from the accessibility organizations on a helper and motion controls.

## Where the research leaves the first position

To be talked through with the maintainer before any decision above is changed.

- **The gyro's rate by a deadzone doesn't stand as written.** A rate can't tell a held hand from one aiming finely, and for a player with a tremor, whom this is for, the idle rate is higher still. What the tools use is engagement by a control: the assist's turning counts while they hold a button or touch something. That is the modes' promise kept by other means, an intervention begun and ended by the assist's own act, and it is what everyone who has built gyro aiming advises.
- **"The primary's accelerometer always" doesn't stand either.** A pad's two sensors want keeping together, or the game's own estimate of gravity and its calibration are misled. The candidates are now: the whole pair from whoever supplies motion at the moment, with the change ramped; or gravity's direction as a weighted mean of the two pads', made 1 g long, which is what gives a game that steers by gravity a true blend.
- **Two things are needed whichever way it goes:** each pad's bias taken out here, by its own stillness, before anything is mixed; and the virtual pad's own clock for its sensor timestamps.
- **The settled practice elsewhere is one source, selected.** Sony's own feature is motion from the primary's pad and none from the assist's. That is the least surprising default for every mode, with the assist's part something a profile opts into.
- **Who has the motion wants showing.** Both studies ask for it, and the tray and the window already say who has control.
- **A profile would need new words for it,** which is the maintainer's thought of widening presets: at least where motion comes from, what engages the assist's, and how gravity is made. The vocabulary above is what such words have been elsewhere. Converting motion into a stick or a mouse is a remapper's work and stays out.

## Decided after the research

By the maintainer, 2026-11-23. These stand over the first position where the two differ.

- **With nothing said in a profile, motion and the touchpad's position are the primary's pad's,** as PlayStation's own assist feature has it: gyroscope, accelerometer and touchpad together from the one pad in priority and average, and from the pad that has control in toggle. The assist's sensors are left out unless a profile asks for them. A game then sees one pad's two sensors, which agree.
- **An assist engages their gyroscope by touching a control the profile names,** not by turning past a rate. While it is held, the assist's motion stands in for the primary's.
- **The order of work:** the plan's decisions now; a spike once the review's fixes have settled; the build after 0.5.0.

Still open: how far presets widen to say this; how gravity is made while the assist's gyroscope is engaged and in average; the touchpad in average; steering by tilt outside toggle; the desktop's pointer.

## What the spike found

`spikes/motion` carried one real DualSense on USB, hidden by the installed hiding service, to a virtual DualSense: buttons and sticks from its event node, both sensors, their timestamp, the touchpad's contacts and click from its raw report. Run on 2026-11-23, on Linux 7.3. A game was not tried; the numbers are from the spike's own record of every report, about 350,000 of them.

- **A virtual DualSense's sensors are taken for a pad's.** The kernel's driver made the motion, touchpad and headset nodes and its motion node followed the pad, seen in jstest-gtk; Steam's controller test showed the sensors and its motion calibration ran to its end as with a real pad.
- **The pad's reports come every 4.00 ms on USB,** by its own clock and as they arrive, the two differing by 0.13 ms from report to report. Over turns of 22 s and 47 s the angle summed by the pad's clock and by the arrival time agreed to 0.01°. Which clock the virtual pad stamps with made no difference that could be measured here, with the machine idle; the longest gap between arrivals in the whole run was 9.5 ms.
- **The pad's own calibration of scale is close enough to the usual pad's not to matter.** The three axes' scales differ from the nominal by under 0.4 %. The pad was spun flat on a desk against one straight edge, two full turns one way and one back: with the bias measured at rest taken off, the sums were 716.5° of 720° and 351.4° of 360°, within what turning by hand allows.
- **The bias the pad was calibrated with is wrong, and is the whole of the error.** At rest this pad reads 4.5, 6.7 and 0.2 counts; its calibration says 4, 15 and -5. Taking the stored bias off, as SDL does, drifts 26° a minute one way; taking none off, as the kernel's driver does, drifts 24° a minute the other. The bias measured over 20 s at rest left under 1° a minute. Steam's calibration does this measuring for itself; a game that trusts the pad does not. So the research's point stands as a requirement: bias is measured at rest, per pad, by whoever combines pads, and what the virtual pad then says of its own calibration should be neutral.
- **The compositor holds both touchpads.** With the spike running, KWin's own list of input devices, read over the session bus, had two enabled touchpads for the one pad: the hidden pad's, its node the hiding service's group's alone, and the virtual pad's. A hidden pad's touchpad was seen to drive the pointer, and the virtual pad's is a touchpad like any other, so a finger on the pad moves the pointer by both. How fast the pointer went by eye showed nothing either way: a DualSense's touchpad is faster than a DualShock 4's with no setup at all. A setup that gives its virtual pad a touchpad must keep the hidden pad's from the compositor, by the rule telling libinput to ignore it; whether the virtual pad's own should drive the pointer is the question that was already open.

Not found out: whether a game that reads the pad through SDL, or through Proton, aims and steers as with the real pad; anything on Bluetooth; anything with the machine busy, where the two clocks may part; a DualShock 4, whose report the spike doesn't read.

## Steps

To be written once the open questions are answered. The first will be a spike: one real DualSense's sensors and touchpad carried to a virtual one, tried in a game that aims by gyro and one that steers by tilt.

## Deviations

None yet.
