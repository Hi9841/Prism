import {Audio} from "@remotion/media";
import {TransitionSeries, linearTiming} from "@remotion/transitions";
import {fade} from "@remotion/transitions/fade";
import {AbsoluteFill, interpolate, staticFile} from "remotion";
import {
  DemoScene,
  IntroScene,
  LauncherScene,
  OutroScene,
  QuickAccessScene,
  SettingsScene,
} from "./scenes";
import {ASSETS, SCENES, TOTAL_FRAMES, TRANSITION_FRAMES} from "./timeline.js";

const transitionTiming = linearTiming({durationInFrames: TRANSITION_FRAMES});

export const PrismShowcase = () => (
  <AbsoluteFill>
    <Audio
      src={staticFile(ASSETS.soundtrack)}
      volume={(frame) =>
        interpolate(frame, [0, 24, TOTAL_FRAMES - 45, TOTAL_FRAMES], [0, 0.55, 0.55, 0], {
          extrapolateLeft: "clamp",
          extrapolateRight: "clamp",
        })
      }
    />
    <TransitionSeries>
      <TransitionSeries.Sequence durationInFrames={SCENES[0].duration} premountFor={30}>
        <IntroScene />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition presentation={fade()} timing={transitionTiming} />
      <TransitionSeries.Sequence durationInFrames={SCENES[1].duration} premountFor={30}>
        <LauncherScene />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition presentation={fade()} timing={transitionTiming} />
      <TransitionSeries.Sequence durationInFrames={SCENES[2].duration} premountFor={30}>
        <DemoScene />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition presentation={fade()} timing={transitionTiming} />
      <TransitionSeries.Sequence durationInFrames={SCENES[3].duration} premountFor={30}>
        <QuickAccessScene />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition presentation={fade()} timing={transitionTiming} />
      <TransitionSeries.Sequence durationInFrames={SCENES[4].duration} premountFor={30}>
        <SettingsScene />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition presentation={fade()} timing={transitionTiming} />
      <TransitionSeries.Sequence durationInFrames={SCENES[5].duration} premountFor={30}>
        <OutroScene />
      </TransitionSeries.Sequence>
    </TransitionSeries>
  </AbsoluteFill>
);
