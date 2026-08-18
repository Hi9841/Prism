import { TransitionSeries, linearTiming } from "@remotion/transitions";
import { fade } from "@remotion/transitions/fade";
import { zColor } from "@remotion/zod-types";
import { z } from "zod";
import { CalculateScene } from "./scenes/CalculateScene";
import { CustomizeScene } from "./scenes/CustomizeScene";
import { IntroScene } from "./scenes/IntroScene";
import { OutroScene } from "./scenes/OutroScene";
import { SearchScene } from "./scenes/SearchScene";

export const prismPromoSchema = z.object({
  productName: z.string(),
  tagline: z.string(),
  cta: z.string(),
  accent: zColor(),
});

export type PrismPromoProps = z.infer<typeof prismPromoSchema>;

export const PrismPromo: React.FC<PrismPromoProps> = (props) => {
  return (
    <TransitionSeries>
      <TransitionSeries.Sequence durationInFrames={120} name="Intro">
        <IntroScene {...props} />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition
        presentation={fade()}
        timing={linearTiming({ durationInFrames: 12 })}
      />
      <TransitionSeries.Sequence durationInFrames={180} name="Unified search">
        <SearchScene {...props} />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition
        presentation={fade()}
        timing={linearTiming({ durationInFrames: 12 })}
      />
      <TransitionSeries.Sequence
        durationInFrames={150}
        name="Calculator and paths"
      >
        <CalculateScene {...props} />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition
        presentation={fade()}
        timing={linearTiming({ durationInFrames: 12 })}
      />
      <TransitionSeries.Sequence
        durationInFrames={150}
        name="Taskbar customization"
      >
        <CustomizeScene {...props} />
      </TransitionSeries.Sequence>
      <TransitionSeries.Transition
        presentation={fade()}
        timing={linearTiming({ durationInFrames: 12 })}
      />
      <TransitionSeries.Sequence durationInFrames={150} name="Outro">
        <OutroScene {...props} />
      </TransitionSeries.Sequence>
    </TransitionSeries>
  );
};
