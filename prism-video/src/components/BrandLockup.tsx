import { Img, Interactive, staticFile } from "remotion";

export const BrandLockup: React.FC<{
  name: string;
  accent: string;
  size?: "compact" | "hero";
}> = ({ name, accent, size = "compact" }) => {
  const hero = size === "hero";

  return (
    <Interactive.Div
      name="Prism brand"
      style={{
        display: "flex",
        alignItems: "center",
        gap: hero ? 30 : 18,
      }}
    >
      <Img
        name="Prism icon"
        src={staticFile("prism-icon.png")}
        style={{
          width: hero ? 122 : 68,
          height: hero ? 122 : 68,
          borderRadius: hero ? 28 : 16,
          boxShadow: `0 18px 48px ${accent}35`,
        }}
      />
      <div
        style={{
          color: "#f8f7fb",
          fontSize: hero ? 108 : 52,
          lineHeight: 1,
          fontWeight: 760,
          letterSpacing: 0,
        }}
      >
        {name}
      </div>
    </Interactive.Div>
  );
};
