- My app programmed in Rust needs a logo banner for its GitHub README.md page.
- Please formulate an image prompt that I can later use with Gemini.
- Here is the excerpt from that README.md file for inspiration:

```markdown
# CtrlAssist

**CtrlAssist** brings "controller assist" functionality to Linux gaming by allowing multiple physical controllers to operate as a single virtual input device. This enables collaborative play and customizable gamepad setups, making it easier for players of all ages and abilities to enjoy games together. While similar features exist on modern game consoles, CtrlAssist is an open source project that enhances accessibility for PC gaming, offering additional quality-of-life improvements through virtual input devices on Linux.

## Features

- Combine physical controllers into one virtual gamepad
  - Controllers are assigned as either Primary or Assist
- Customizable multiplexing of buttons and axes
  - Logically merging or preempting events is flexible
- Hide physical controllers for improved game compatibility
  - Avoid controller interference from conflicting inputs
- Spoof gamepad vendor for in-game layout recognition
  - Mimic either Primary or Assist controller hardware
```

---

Here are three distinct image prompt options for your Gemini generation, ranging from literal technical interpretations to mascot-based illustrations.

They are designed to create a wide banner suitable for a GitHub README header.

### Option 1: The Technical "Merge" Metaphor (Clean & Modern)

This option focuses strictly on the core functionality: taking multiple physical inputs and merging them into one virtual output. It feels technical, precise, and "Rust-like."

**Best for:** A professional, engineering-focused look.

**Prompt for Gemini:**

```text
A wide horizontal banner illustration for a software project named "CtrlAssist". The visual metaphor shows two distinct physical game controllers (one black, one white) on the left side, with glowing blue and orange data cables extending from them. These cables extend to the center and fuse together into a single, glowing, stylized, translucent virtual controller icon suspended in the air. Below this virtual icon, a stylized "VIRTUAL INPUT" label glows. On the right side, the text "CtrlAssist" is rendered in a bold, industrial, modern sans-serif font, with a subtitle below it reading: "Collaborative Controller Fusion for Linux". The background is a clean, dark technical blueprint aesthetic with subtle circuit board patterns and a color palette of deep blues, oranges, and charcoal greys, accented with a subtle Rust-language gear motif in the corner. The overall aspect ratio is 3:1.
```

### Option 2: The Collaborative/Accessibility Focus (Warm & Approachable)

This option emphasizes the human aspect of the project: helping people play together, regardless of ability. It's warmer and more illustrative.

**Best for:** Highlighting the accessibility and co-op mission of the project.

**Prompt for Gemini:**

```text
A horizontal banner illustration rendered in a friendly, warm, digital painting style. The center of the image features a single, large game controller being held simultaneously by two pairs of stylized hands: one pair is larger, perhaps robotic or metallic (representing the 'Primary' or technical side), and the other pair is smaller, softer, and glowing gently (representing the 'Assist' side). Both pairs are cooperating to press buttons on the same device. Above the hands, the title "CtrlAssist" is written in a friendly, rounded, yet sturdy font. Below it, a smaller tagline reads: "Shared Control. Accessible Gaming." The background is a soft gradient of inviting purples and teals with subtle, stylized Linux penguin (Tux) silhouettes and gear icons floating in the distance. The aspect ratio is wide, suitable for a web header.
```

### Option 3: The Mascot Mashup (Fun & Open Source Specific)

In the open-source world, mascot collaboration is a popular trope. This option uses the Rust mascot (Ferris the crab) and the Linux mascot (Tux the penguin) working together.

**Best for:** A fun, community-oriented vibe that immediately signals the tech stack.

**Prompt for Gemini:**

```text
A cheerful, wide banner illustration in a clean cartoon style. On the left side, Ferris the Rust crab (a cute orange crab) and Tux the Linux penguin are sitting side-by-side on a couch. They are both happily holding onto a single, oversized game controller together; Ferris is manipulating the left joystick with a claw, and Tux is pressing the right buttons with a flipper. Wires run from the controller into a central hub box labeled "CtrlAssist Core" which emits a cheerful green glow. Large, playful text across the top right reads "CtrlAssist". Below it, smaller text reads: "Rust-Powered Co-op for Linux". The background is a stylized, cozy living room with a terminal window screen faintly visible on a wall. The aspect ratio is 21:9.
```

---

Could you combine Option 1 and Option 3? I like idea of Ferris the Rust crab and Tux the Linux penguin playing collaboratively from Option 3, but I'd instead want them using two separate orange and blue controllers from Option 1.

---

Here is the combined prompt, blending the mascot cooperation with the two-into-one technical metaphor.

**Best for:** A fun, clear visual explanation of how the open-source tech works using beloved community icons.

**Prompt for Gemini:**

```text
A wide horizontal banner illustration in a clean, vibrant cartoon style with technological elements. On a comfortable gaming couch on the left side sits Ferris the Rust crab (the orange mascot), enthusiastically holding a distinct physical orange game controller. On the right side of the couch sits Tux the Linux penguin, happily holding a distinct physical blue game controller. Both are looking at a central point between them.

From Ferris's orange controller, a glowing orange data cable extends towards the center. From Tux's blue controller, a glowing blue data cable extends towards the center. These two colored cables converge and plug into a sleek, central hardware hub console situated on a coffee table, labeled "CtrlAssist Core".

Emerging from the top of this central hub is a single, large, unified, glowing holographic projection of a virtual gamepad (perhaps a blend of purple and white light), indicating the merged output. In the background, a large TV screen shows a game being played smoothly. The overall background is a cozy living room with subtle circuit board patterns on the walls.

At the top center, the title "CtrlAssist" is rendered in a bold, friendly tech font. Below it, a tagline reads: "Two physical controllers, one virtual input. Rust & Linux playing together." The aspect ratio is 3:1 suitable for a header.
```
