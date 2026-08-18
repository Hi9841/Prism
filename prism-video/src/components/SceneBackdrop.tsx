import { AbsoluteFill, Interactive } from "remotion";

export const SceneBackdrop: React.FC<{
  accent: string;
  children: React.ReactNode;
  section: string;
}> = ({ accent, children, section }) => {
  return (
    <AbsoluteFill
      style={{
        backgroundColor: "#0b0d12",
        color: "#f5f4fa",
        fontFamily:
          '"Segoe UI Variable Display", "Segoe UI", Arial, sans-serif',
        overflow: "hidden",
      }}
    >
      <div
        style={{
          position: "absolute",
          inset: 0,
          opacity: 0.42,
          backgroundImage:
            "linear-gradient(rgba(255,255,255,0.035) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.035) 1px, transparent 1px)",
          backgroundSize: "72px 72px",
        }}
      />
      <div
        style={{
          position: "absolute",
          top: 0,
          left: 0,
          width: 10,
          height: "100%",
          backgroundColor: accent,
        }}
      />
      <div
        style={{
          position: "absolute",
          top: 74,
          right: 82,
          width: 420,
          height: 1,
          backgroundColor: "rgba(255,255,255,0.12)",
        }}
      />
      <Interactive.Div
        name="Scene label"
        style={{
          position: "absolute",
          top: 48,
          right: 82,
          color: "rgba(245,244,250,0.48)",
          fontSize: 20,
          fontWeight: 650,
          letterSpacing: 0,
          textTransform: "uppercase",
        }}
      >
        {section}
      </Interactive.Div>
      {children}
    </AbsoluteFill>
  );
};
