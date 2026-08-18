import {
  Easing,
  Interactive,
  interpolate,
  Sequence,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";
import type { PrismPromoProps } from "../PrismPromo";
import { PrismWindow } from "../components/PrismWindow";
import { SceneBackdrop } from "../components/SceneBackdrop";

const CalculatorDemo: React.FC<{ accent: string }> = ({ accent }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const query = "128 * 24".slice(
    0,
    Math.round(
      interpolate(frame, [0.15 * fps, 0.9 * fps], [0, 8], {
        extrapolateLeft: "clamp",
        extrapolateRight: "clamp",
      }),
    ),
  );

  return (
    <Interactive.Div
      name="Calculator demo"
      style={{
        position: "absolute",
        left: 722,
        top: 214,
        scale: 0.92,
        opacity: interpolate(
          frame,
          [0, 0.4 * fps, 2.15 * fps, 2.45 * fps],
          [0, 1, 1, 0],
          {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: [
              Easing.bezier(0.16, 1, 0.3, 1),
              Easing.linear,
              Easing.bezier(0.7, 0, 0.84, 0),
            ],
          },
        ),
        translate: interpolate(frame, [0, 0.6 * fps], ["80px 0px", "0px 0px"], {
          extrapolateLeft: "clamp",
          extrapolateRight: "clamp",
          easing: Easing.bezier(0.16, 1, 0.3, 1),
        }),
      }}
    >
      <PrismWindow
        accent={accent}
        query={query}
        footer="Enter  Copy result"
        results={[
          {
            kind: "calc",
            title: "3,072",
            subtitle: "128 × 24",
            action: "Enter to copy",
          },
          {
            kind: "calc",
            title: "128 × 24 = 3,072",
            subtitle: "Calculation history",
          },
          { kind: "file", title: "Q4 budget.xlsx", subtitle: "Documents" },
          {
            kind: "folder",
            title: "Finance",
            subtitle: "C:\\Users\\hi\\Documents\\Finance",
          },
        ]}
      />
    </Interactive.Div>
  );
};

const PathDemo: React.FC<{ accent: string }> = ({ accent }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const fullQuery = "C:\\Users\\hi\\Downloads";
  const query = fullQuery.slice(
    0,
    Math.round(
      interpolate(frame, [0, 1.2 * fps], [0, fullQuery.length], {
        extrapolateLeft: "clamp",
        extrapolateRight: "clamp",
      }),
    ),
  );

  return (
    <Interactive.Div
      name="Path demo"
      style={{
        position: "absolute",
        left: 722,
        top: 214,
        scale: 0.92,
        opacity: interpolate(frame, [0, 0.42 * fps], [0, 1], {
          extrapolateLeft: "clamp",
          extrapolateRight: "clamp",
          easing: Easing.bezier(0.16, 1, 0.3, 1),
        }),
        translate: interpolate(
          frame,
          [0, 0.65 * fps],
          ["60px 0px", "0px 0px"],
          {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          },
        ),
      }}
    >
      <PrismWindow
        accent={accent}
        query={query}
        footer="Enter  Open folder"
        results={[
          {
            kind: "folder",
            title: "Downloads",
            subtitle: "C:\\Users\\hi\\Downloads",
            action: "Open folder",
          },
          {
            kind: "file",
            title: "Prism_0.9.8_x64-setup.exe",
            subtitle: "Downloaded today",
          },
          {
            kind: "image",
            title: "prism-preview.png",
            subtitle: "Images · 1920 × 1080",
          },
          {
            kind: "folder",
            title: "Installers",
            subtitle: "C:\\Users\\hi\\Downloads\\Installers",
          },
        ]}
      />
    </Interactive.Div>
  );
};

export const CalculateScene: React.FC<PrismPromoProps> = ({ accent }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  return (
    <SceneBackdrop accent={accent} section="More than a launcher">
      <Interactive.Div
        name="Utility headline"
        style={{
          position: "absolute",
          left: 108,
          top: 235,
          width: 520,
          color: "#f7f6fa",
          fontSize: 88,
          lineHeight: 1.02,
          fontWeight: 760,
          letterSpacing: 0,
          opacity: interpolate(frame, [0, 0.45 * fps], [0, 1], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          }),
          translate: interpolate(
            frame,
            [0, 0.55 * fps],
            ["0px 40px", "0px 0px"],
            {
              extrapolateLeft: "clamp",
              extrapolateRight: "clamp",
              easing: Easing.bezier(0.16, 1, 0.3, 1),
            },
          ),
        }}
      >
        Think.
        <br />
        Find.
        <br />
        Open.
      </Interactive.Div>
      <Interactive.Div
        name="Utility caption"
        style={{
          position: "absolute",
          left: 112,
          top: 555,
          width: 490,
          color: "#a8a5b1",
          fontSize: 32,
          lineHeight: 1.35,
          fontWeight: 520,
          letterSpacing: 0,
        }}
      >
        {frame < 66
          ? "Calculate without leaving your flow."
          : "Browse a path as quickly as you can type it."}
      </Interactive.Div>
      <div
        style={{
          position: "absolute",
          left: 111,
          top: 695,
          height: 48,
          padding: "0 18px",
          borderRadius: 24,
          display: "flex",
          alignItems: "center",
          color: accent,
          backgroundColor: `${accent}18`,
          border: `1px solid ${accent}55`,
          fontSize: 20,
          fontWeight: 700,
        }}
      >
        {frame < 66 ? "Enter to copy" : "Enter to open"}
      </div>

      <Sequence durationInFrames={82} layout="none">
        <CalculatorDemo accent={accent} />
      </Sequence>
      <Sequence from={58} durationInFrames={92} layout="none">
        <PathDemo accent={accent} />
      </Sequence>
    </SceneBackdrop>
  );
};
