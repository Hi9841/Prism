import {Composition} from "remotion";
import {PrismShowcase} from "./PrismShowcase";
import {FPS, TOTAL_FRAMES} from "./timeline.js";

export const RemotionRoot = () => (
  <Composition
    id="PrismShowcase"
    component={PrismShowcase}
    durationInFrames={TOTAL_FRAMES}
    fps={FPS}
    width={1920}
    height={1080}
  />
);
