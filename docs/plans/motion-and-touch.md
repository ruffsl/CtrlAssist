# Motion sensors and the touchpad

## Goal

A PlayStation pad has three kinds of input beyond its buttons, sticks and triggers: a touchpad that reports where fingers are, a gyroscope that reports how fast the pad is turning, and an accelerometer that reports which way is down and how the pad is shaken. The kernel gives each its own node beside the pad's. CtrlAssist reads none of them. The touchpad's click is a button and is routed as one; a virtual DualSense has a touchpad and sensors that report nothing.

That was a choice of scope: the routing model is built for controls that rest at zero. It has become a loss. A hidden pad now has every one of its nodes hidden, so that a game sees one controller and not a second, partial one; with that, a game can no longer read the real pad's sensors or touchpad for itself, and nothing carries them to the virtual pad. While a setup hides a PlayStation pad, gyro aiming, tilt and touchpad gestures are gone.

The goal is for a virtual DualSense to carry them, by rules that keep what each mode promises: the primary player stays in charge, and what the assist does is bounded in time and in scope.

## State

Not started. The decisions below are a first position, taken in conversation on 2026-11-23, and wait on the research in Open questions before any is settled. Nothing is to be built before 0.5.0 unless the maintainer says so.

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
- **The desktop's use of the touchpad.** Outside a game, a DualSense's touchpad moves the desktop's pointer, and Steam can map it. While a setup hides the pad its touchpad's nodes are hidden with it. Whether the pointer then stops was not checked through a compositor, only that the seat user can't open the nodes; and a pad that is grabbed and not hidden keeps its touchpad, a grab being of the pad's own node alone. Whether a setup should leave the desktop its pointer is a question of its own.
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

## Steps

To be written once the open questions are answered. The first will be a spike: one real DualSense's sensors and touchpad carried to a virtual one, tried in a game that aims by gyro and one that steers by tilt.

## Deviations

None yet.
