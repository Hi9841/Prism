import { Composition, Folder } from "remotion";
import { PrismPromo, prismPromoSchema } from "./PrismPromo";
import { CalculateScene } from "./scenes/CalculateScene";
import { CustomizeScene } from "./scenes/CustomizeScene";
import { IntroScene } from "./scenes/IntroScene";
import { OutroScene } from "./scenes/OutroScene";
import { SearchScene } from "./scenes/SearchScene";

export const RemotionRoot: React.FC = () => {
  return (
    <>
      <Composition
        id="PrismPromo-16x9"
        component={PrismPromo}
        durationInFrames={702}
        fps={30}
        width={1920}
        height={1080}
        schema={prismPromoSchema}
        defaultProps={{
          productName: "Prism",
          tagline: "Your Windows workflow, one shortcut away.",
          cta: "Free for Windows 10 & 11",
          accent: "#8f7cf5",
        }}
      />
      <Folder name="Prism-Promo-Scenes">
        <Composition
          id="Prism-01-Intro"
          component={IntroScene}
          durationInFrames={120}
          fps={30}
          width={1920}
          height={1080}
          schema={prismPromoSchema}
          defaultProps={{
            productName: "Prism",
            tagline: "Your Windows workflow, one shortcut away.",
            cta: "Free for Windows 10 & 11",
            accent: "#8f7cf5",
          }}
        />
        <Composition
          id="Prism-02-Search"
          component={SearchScene}
          durationInFrames={180}
          fps={30}
          width={1920}
          height={1080}
          schema={prismPromoSchema}
          defaultProps={{
            productName: "Prism",
            tagline: "Your Windows workflow, one shortcut away.",
            cta: "Free for Windows 10 & 11",
            accent: "#8f7cf5",
          }}
        />
        <Composition
          id="Prism-03-Calculate"
          component={CalculateScene}
          durationInFrames={150}
          fps={30}
          width={1920}
          height={1080}
          schema={prismPromoSchema}
          defaultProps={{
            productName: "Prism",
            tagline: "Your Windows workflow, one shortcut away.",
            cta: "Free for Windows 10 & 11",
            accent: "#8f7cf5",
          }}
        />
        <Composition
          id="Prism-04-Customize"
          component={CustomizeScene}
          durationInFrames={150}
          fps={30}
          width={1920}
          height={1080}
          schema={prismPromoSchema}
          defaultProps={{
            productName: "Prism",
            tagline: "Your Windows workflow, one shortcut away.",
            cta: "Free for Windows 10 & 11",
            accent: "#8f7cf5",
          }}
        />
        <Composition
          id="Prism-05-Outro"
          component={OutroScene}
          durationInFrames={150}
          fps={30}
          width={1920}
          height={1080}
          schema={prismPromoSchema}
          defaultProps={{
            productName: "Prism",
            tagline: "Your Windows workflow, one shortcut away.",
            cta: "Free for Windows 10 & 11",
            accent: "#8f7cf5",
          }}
        />
      </Folder>
    </>
  );
};
