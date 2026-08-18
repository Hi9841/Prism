import { Check, GitFork, Monitor } from "lucide-react";
import {
  Easing,
  Img,
  Interactive,
  interpolate,
  staticFile,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";
import type { PrismPromoProps } from "../PrismPromo";
import { SceneBackdrop } from "../components/SceneBackdrop";

export const OutroScene: React.FC<PrismPromoProps> = ({
  productName,
  cta,
  accent,
}) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  return (
    <SceneBackdrop accent={accent} section="Ready when you are">
      <Interactive.Div
        name="Outro icon"
        style={{
          position: "absolute",
          left: 220,
          top: 250,
          opacity: interpolate(frame, [0, 0.5 * fps], [0, 1], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          }),
          scale: interpolate(frame, [0, 0.85 * fps], [0.72, 1], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.spring({ damping: 170 }),
            output: "perceptual-scale",
          }),
          rotate: interpolate(frame, [0, 0.9 * fps], ["-5deg", "0deg"], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.spring({ damping: 170 }),
          }),
        }}
      >
        <Img
          name="Prism icon"
          src={staticFile("prism-icon.png")}
          style={{
            width: 340,
            height: 340,
            borderRadius: 76,
            boxShadow: `0 36px 100px ${accent}40`,
          }}
        />
      </Interactive.Div>

      <Interactive.Div
        name="Outro copy"
        style={{
          position: "absolute",
          left: 700,
          top: 220,
          width: 990,
          opacity: interpolate(frame, [0.25 * fps, 0.8 * fps], [0, 1], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          }),
          translate: interpolate(
            frame,
            [0.2 * fps, 0.95 * fps],
            ["0px 48px", "0px 0px"],
            {
              extrapolateLeft: "clamp",
              extrapolateRight: "clamp",
              easing: Easing.bezier(0.16, 1, 0.3, 1),
            },
          ),
        }}
      >
        <Interactive.Div
          name="Outro title"
          style={{
            color: "#f8f7fb",
            fontSize: 142,
            lineHeight: 0.98,
            fontWeight: 780,
            letterSpacing: 0,
          }}
        >
          Meet {productName}.
        </Interactive.Div>
        <Interactive.Div
          name="Outro statement"
          style={{
            marginTop: 34,
            color: "#b2afbb",
            fontSize: 42,
            lineHeight: 1.25,
            fontWeight: 520,
            letterSpacing: 0,
            maxWidth: 900,
          }}
        >
          A fast command palette and Windows taskbar companion.
        </Interactive.Div>

        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 16,
            marginTop: 54,
          }}
        >
          <div
            style={{
              height: 62,
              padding: "0 24px",
              borderRadius: 12,
              display: "flex",
              alignItems: "center",
              gap: 12,
              color: "#15131d",
              backgroundColor: accent,
              fontSize: 22,
              fontWeight: 760,
              boxShadow: `0 14px 36px ${accent}2f`,
            }}
          >
            <Check size={24} strokeWidth={2.4} />
            {cta}
          </div>
          <div
            style={{
              height: 62,
              padding: "0 22px",
              borderRadius: 12,
              display: "flex",
              alignItems: "center",
              gap: 11,
              color: "#d0cdd7",
              backgroundColor: "rgba(255,255,255,0.05)",
              border: "1px solid rgba(255,255,255,0.1)",
              fontSize: 21,
              fontWeight: 660,
            }}
          >
            <GitFork size={24} strokeWidth={1.8} />
            github.com/Hi9841/Prism
          </div>
        </div>

        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            marginTop: 31,
            color: "#85828e",
          }}
        >
          <Monitor size={21} strokeWidth={1.7} />
          <span style={{ fontSize: 19, fontWeight: 580 }}>
            Built for Windows 10 and Windows 11
          </span>
        </div>
      </Interactive.Div>
    </SceneBackdrop>
  );
};
