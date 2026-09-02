export const FPS = 30;
export const TRANSITION_FRAMES = 10;

export const SCENES = [
  {id: "intro", duration: 84},
  {id: "launcher", duration: 135},
  {id: "demo", duration: 183},
  {id: "quick-access", duration: 123},
  {id: "settings", duration: 126},
  {id: "outro", duration: 90},
];

export const TOTAL_FRAMES =
  SCENES.reduce((sum, scene) => sum + scene.duration, 0) -
  TRANSITION_FRAMES * (SCENES.length - 1);

export const ASSETS = {
  icon: "prism-icon.png",
  launcher: "prism-current.png",
  quickAccess: "prism-quick-access-real.png",
  settings: "prism-settings-real.png",
  demo: "prism-demo.mp4",
  soundtrack: "prism-ambient.wav",
};
