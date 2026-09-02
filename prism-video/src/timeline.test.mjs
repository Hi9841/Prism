import assert from "node:assert/strict";
import test from "node:test";

import {
  ASSETS,
  FPS,
  SCENES,
  TOTAL_FRAMES,
  TRANSITION_FRAMES,
} from "./timeline.js";

test("the composition resolves to a 691-frame product cut", () => {
  const sceneFrames = SCENES.reduce((sum, scene) => sum + scene.duration, 0);
  const overlapFrames = TRANSITION_FRAMES * (SCENES.length - 1);

  assert.equal(FPS, 30);
  assert.equal(TOTAL_FRAMES, sceneFrames - overlapFrames);
  assert.equal(TOTAL_FRAMES, 691);
});

test("every scene has enough time to read at 30 fps", () => {
  for (const scene of SCENES) {
    assert.ok(scene.duration >= 84, `${scene.id} is shorter than 2.8 seconds`);
  }
});

test("the showcase uses only current real-product assets", () => {
  assert.deepEqual(ASSETS, {
    icon: "prism-icon.png",
    launcher: "prism-current.png",
    quickAccess: "prism-quick-access-real.png",
    settings: "prism-settings-real.png",
    demo: "prism-demo.mp4",
    soundtrack: "prism-ambient.wav",
  });
});
