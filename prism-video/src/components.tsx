import type {CSSProperties, ReactNode} from "react";
import {
  AbsoluteFill,
  Easing,
  Img,
  interpolate,
  staticFile,
  useCurrentFrame,
} from "remotion";

export const COLORS = {
  canvas: "#090a0f",
  panel: "#111219",
  panelRaised: "#171820",
  paper: "#f4f3f8",
  muted: "#a8a7b2",
  faint: "#6e6d78",
  line: "#2c2d36",
  violet: "#8e78ff",
  violetStrong: "#7058ed",
  mint: "#58d6b2",
  amber: "#f3b84b",
} as const;

const easeOut = Easing.bezier(0.16, 1, 0.3, 1);

export const reveal = (frame: number, start = 0, duration = 24) =>
  interpolate(frame, [start, start + duration], [0, 1], {
    easing: easeOut,
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

export const SceneFrame = ({
  children,
  label,
  labelColor = COLORS.violet,
}: {
  children: ReactNode;
  label: string;
  labelColor?: string;
}) => {
  const frame = useCurrentFrame();
  const topLine = reveal(frame, 4, 28);

  return (
    <AbsoluteFill
      style={{
        backgroundColor: COLORS.canvas,
        color: COLORS.paper,
        fontFamily: '"Segoe UI Variable", "Segoe UI", Arial, sans-serif',
        overflow: "hidden",
      }}
    >
      <div
        style={{
          position: "absolute",
          left: 0,
          top: 0,
          bottom: 0,
          width: 8,
          backgroundColor: labelColor,
        }}
      />
      <div
        style={{
          position: "absolute",
          left: 112,
          right: 112,
          top: 68,
          height: 1,
          backgroundColor: COLORS.line,
          transform: `scaleX(${topLine})`,
          transformOrigin: "right center",
        }}
      />
      <div
        style={{
          position: "absolute",
          top: 39,
          right: 112,
          color: labelColor,
          fontSize: 17,
          fontWeight: 700,
          letterSpacing: 0,
          textTransform: "uppercase",
          opacity: topLine,
        }}
      >
        {label}
      </div>
      {children}
    </AbsoluteFill>
  );
};

export const Reveal = ({
  children,
  delay = 0,
  distance = 26,
  style,
}: {
  children: ReactNode;
  delay?: number;
  distance?: number;
  style?: CSSProperties;
}) => {
  const frame = useCurrentFrame();
  const progress = reveal(frame, delay, 24);

  return (
    <div
      style={{
        opacity: progress,
        transform: `translateY(${(1 - progress) * distance}px)`,
        ...style,
      }}
    >
      {children}
    </div>
  );
};

export const ProductImage = ({
  src,
  width,
  style,
}: {
  src: string;
  width: number;
  style?: CSSProperties;
}) => {
  const frame = useCurrentFrame();
  const progress = reveal(frame, 8, 32);

  return (
    <div
      style={{
        width,
        border: `1px solid ${COLORS.line}`,
        borderRadius: 8,
        overflow: "hidden",
        backgroundColor: COLORS.panel,
        boxShadow: "0 34px 90px rgba(0, 0, 0, 0.46)",
        opacity: progress,
        transform: `translateY(${(1 - progress) * 42}px) scale(${0.975 + progress * 0.025})`,
        ...style,
      }}
    >
      <Img src={staticFile(src)} style={{display: "block", width: "100%", height: "auto"}} />
    </div>
  );
};

export const Keycap = ({children}: {children: ReactNode}) => (
  <span
    style={{
      display: "inline-flex",
      minWidth: 58,
      height: 42,
      padding: "0 15px",
      alignItems: "center",
      justifyContent: "center",
      border: `1px solid ${COLORS.line}`,
      borderBottomColor: "#4a4b57",
      borderRadius: 6,
      backgroundColor: COLORS.panelRaised,
      color: COLORS.paper,
      fontSize: 16,
      fontWeight: 700,
    }}
  >
    {children}
  </span>
);

export const FeaturePill = ({children, accent = COLORS.violet}: {children: ReactNode; accent?: string}) => (
  <span
    style={{
      display: "inline-flex",
      alignItems: "center",
      gap: 10,
      minHeight: 42,
      padding: "0 17px",
      border: `1px solid ${COLORS.line}`,
      borderRadius: 6,
      backgroundColor: COLORS.panel,
      color: COLORS.paper,
      fontSize: 17,
      fontWeight: 600,
    }}
  >
    <span style={{width: 7, height: 7, borderRadius: "50%", backgroundColor: accent}} />
    {children}
  </span>
);

export const Logo = ({size}: {size: number}) => (
  <Img
    src={staticFile("prism-icon.png")}
    style={{width: size, height: size, objectFit: "contain", borderRadius: Math.round(size * 0.22)}}
  />
);
