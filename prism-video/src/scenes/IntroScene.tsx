import {
  Easing,
  Interactive,
  interpolate,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";
import type { PrismPromoProps } from "../PrismPromo";
import { BrandLockup } from "../components/BrandLockup";
import { PrismWindow } from "../components/PrismWindow";
import { SceneBackdrop } from "../components/SceneBackdrop";

export const IntroScene: React.FC<PrismPromoProps> = ({
  productName,
  tagline,
  accent,
}) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  return (
    <SceneBackdrop accent={accent} section="Windows, refined">
      <Interactive.Div
        name="Intro copy"
        style={{
          position: "absolute",
          left: 108,
          top: 170,
          width: 750,
          opacity: interpolate(frame, [0, 0.55 * fps], [0, 1], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          }),
          translate: interpolate(
            frame,
            [0, 0.7 * fps],
            ["0px 48px", "0px 0px"],
            {
              extrapolateLeft: "clamp",
              extrapolateRight: "clamp",
              easing: Easing.bezier(0.16, 1, 0.3, 1),
            },
          ),
        }}
      >
        <BrandLockup name={productName} accent={accent} size="hero" />
        <Interactive.Div
          name="Intro tagline"
          style={{
            marginTop: 48,
            color: "#d1cedb",
            fontSize: 54,
            lineHeight: 1.16,
            fontWeight: 560,
            letterSpacing: 0,
            maxWidth: 700,
          }}
        >
          {tagline}
        </Interactive.Div>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 12,
            marginTop: 48,
          }}
        >
          {["WIN", "+", "SPACE"].map((key) => (
            <div
              key={key}
              style={{
                minWidth: key === "+" ? 22 : 78,
                height: key === "+" ? 48 : 54,
                padding: key === "+" ? 0 : "0 16px",
                borderRadius: key === "+" ? 0 : 10,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                color: key === "+" ? "#777582" : "#d8d6df",
                fontSize: key === "+" ? 24 : 18,
                fontWeight: 700,
                backgroundColor:
                  key === "+" ? "transparent" : "rgba(255,255,255,0.06)",
                border:
                  key === "+" ? "none" : "1px solid rgba(255,255,255,0.11)",
                boxShadow:
                  key === "+" ? "none" : "inset 0 -3px 0 rgba(0,0,0,0.28)",
              }}
            >
              {key}
            </div>
          ))}
          <div
            style={{
              marginLeft: 8,
              color: accent,
              fontSize: 22,
              fontWeight: 650,
            }}
          >
            Open from anywhere
          </div>
        </div>
      </Interactive.Div>

      <Interactive.Div
        name="Intro product preview"
        style={{
          position: "absolute",
          left: 930,
          top: 246,
          scale: interpolate(frame, [0.25 * fps, 1.2 * fps], [0.88, 0.72], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.spring({ damping: 180 }),
            output: "perceptual-scale",
          }),
          opacity: interpolate(frame, [0.15 * fps, 0.7 * fps], [0, 1], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          }),
          rotate: interpolate(frame, [0.2 * fps, 1.1 * fps], ["2deg", "0deg"], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.spring({ damping: 180 }),
          }),
        }}
      >
        <PrismWindow
          accent={accent}
          query=""
          results={[
            {
              kind: "app",
              title: "Visual Studio Code",
              subtitle: "Application",
              action: "Enter to open",
            },
            {
              kind: "folder",
              title: "Projects",
              subtitle: "C:\\Users\\hi\\Projects",
            },
            {
              kind: "file",
              title: "Launch checklist.md",
              subtitle: "Documents",
            },
            {
              kind: "calc",
              title: "128 × 24 = 3,072",
              subtitle: "Calculation",
              action: "Enter to copy",
            },
          ]}
        />
      </Interactive.Div>
    </SceneBackdrop>
  );
};
