import {
  Easing,
  Interactive,
  interpolate,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";
import type { PrismPromoProps } from "../PrismPromo";
import { PrismWindow } from "../components/PrismWindow";
import { SceneBackdrop } from "../components/SceneBackdrop";

export const SearchScene: React.FC<PrismPromoProps> = ({ accent }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const query = "prism".slice(
    0,
    Math.round(
      interpolate(frame, [0.45 * fps, 1.45 * fps], [0, 5], {
        extrapolateLeft: "clamp",
        extrapolateRight: "clamp",
      }),
    ),
  );

  return (
    <SceneBackdrop accent={accent} section="Unified search">
      <Interactive.Div
        name="Search headline"
        style={{
          position: "absolute",
          left: 108,
          top: 108,
          color: "#f7f6fa",
          fontSize: 92,
          lineHeight: 1,
          fontWeight: 760,
          letterSpacing: 0,
          opacity: interpolate(frame, [0, 0.5 * fps], [0, 1], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          }),
          translate: interpolate(
            frame,
            [0, 0.6 * fps],
            ["0px 36px", "0px 0px"],
            {
              extrapolateLeft: "clamp",
              extrapolateRight: "clamp",
              easing: Easing.bezier(0.16, 1, 0.3, 1),
            },
          ),
        }}
      >
        Search everything.
      </Interactive.Div>
      <Interactive.Div
        name="Search subtitle"
        style={{
          position: "absolute",
          left: 111,
          top: 214,
          color: "#a8a5b1",
          fontSize: 34,
          fontWeight: 520,
          letterSpacing: 0,
          opacity: interpolate(frame, [0.3 * fps, 0.8 * fps], [0, 1], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          }),
        }}
      >
        Apps, local files, and folders in one keyboard-first flow.
      </Interactive.Div>

      <Interactive.Div
        name="Search demo"
        style={{
          position: "absolute",
          left: 430,
          top: 325,
          scale: 1.02,
          opacity: interpolate(frame, [0.2 * fps, 0.85 * fps], [0, 1], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          }),
          translate: interpolate(
            frame,
            [0.15 * fps, 1 * fps],
            ["0px 70px", "0px 0px"],
            {
              extrapolateLeft: "clamp",
              extrapolateRight: "clamp",
              easing: Easing.spring({ damping: 190 }),
            },
          ),
        }}
      >
        <PrismWindow
          accent={accent}
          query={query}
          selected={frame < 82 ? 0 : frame < 112 ? 1 : 2}
          results={[
            {
              kind: "app",
              title: "Prism",
              subtitle: "Application",
              action: "Enter to open",
            },
            {
              kind: "folder",
              title: "Prism",
              subtitle: "C:\\Users\\hi\\Desktop\\Work\\Prism",
            },
            {
              kind: "file",
              title: "Prism release notes.md",
              subtitle: "Documents · Modified today",
            },
            {
              kind: "image",
              title: "prism-icon.png",
              subtitle: "Images · 512 × 512",
            },
          ]}
        />
      </Interactive.Div>

      <div
        style={{
          position: "absolute",
          right: 120,
          bottom: 62,
          display: "flex",
          gap: 10,
        }}
      >
        {["Applications", "Files", "Folders"].map((label, index) => (
          <div
            key={label}
            style={{
              height: 42,
              padding: "0 18px",
              borderRadius: 21,
              display: "flex",
              alignItems: "center",
              color: index === 0 ? accent : "#9c99a5",
              backgroundColor:
                index === 0 ? `${accent}19` : "rgba(255,255,255,0.04)",
              border: `1px solid ${index === 0 ? `${accent}55` : "rgba(255,255,255,0.08)"}`,
              fontSize: 18,
              fontWeight: 650,
            }}
          >
            {label}
          </div>
        ))}
      </div>
    </SceneBackdrop>
  );
};
