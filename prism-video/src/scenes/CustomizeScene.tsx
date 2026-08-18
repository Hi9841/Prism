import {
  AlignCenter,
  AlignLeft,
  AppWindow,
  Check,
  Gem,
  Monitor,
  Palette,
  Settings2,
} from "lucide-react";
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

const SegmentedOption: React.FC<{
  active: boolean;
  accent: string;
  label: string;
  icon: React.ReactNode;
}> = ({ active, accent, label, icon }) => (
  <div
    style={{
      height: 48,
      padding: "0 16px",
      borderRadius: 10,
      display: "flex",
      alignItems: "center",
      gap: 9,
      color: active ? "#f4f3f7" : "#8e8b97",
      backgroundColor: active ? "rgba(255,255,255,0.09)" : "transparent",
      border: active ? `1px solid ${accent}55` : "1px solid transparent",
      fontSize: 18,
      fontWeight: 650,
    }}
  >
    {icon}
    {label}
  </div>
);

export const CustomizeScene: React.FC<PrismPromoProps> = ({ accent }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const leftAligned = frame >= 75;

  return (
    <SceneBackdrop accent={accent} section="Taskbar companion">
      <Interactive.Div
        name="Customize headline"
        style={{
          position: "absolute",
          left: 108,
          top: 150,
          width: 690,
          color: "#f7f6fa",
          fontSize: 92,
          lineHeight: 1.02,
          fontWeight: 760,
          letterSpacing: 0,
          opacity: interpolate(frame, [0, 0.5 * fps], [0, 1], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          }),
        }}
      >
        Your taskbar.
        <br />
        Your rules.
      </Interactive.Div>
      <Interactive.Div
        name="Customize subtitle"
        style={{
          position: "absolute",
          left: 112,
          top: 365,
          width: 565,
          color: "#aaa7b3",
          fontSize: 33,
          lineHeight: 1.34,
          fontWeight: 520,
          letterSpacing: 0,
        }}
      >
        Alignment, icon density, grouping, auto-hide, and your own Start icon.
      </Interactive.Div>
      <div
        style={{
          position: "absolute",
          left: 112,
          top: 545,
          display: "grid",
          gridTemplateColumns: "1fr 1fr",
          gap: 12,
          width: 560,
        }}
      >
        {[
          [Palette, "Five accents"],
          [Monitor, "Windows theme"],
          [AppWindow, "Icon density"],
          [Gem, "Custom Start"],
        ].map(([Icon, label]) => {
          const FeatureIcon = Icon as typeof Palette;
          return (
            <div
              key={String(label)}
              style={{
                height: 66,
                padding: "0 16px",
                borderRadius: 12,
                display: "flex",
                alignItems: "center",
                gap: 12,
                color: "#c9c6d1",
                backgroundColor: "rgba(255,255,255,0.04)",
                border: "1px solid rgba(255,255,255,0.08)",
                fontSize: 19,
                fontWeight: 650,
              }}
            >
              <FeatureIcon size={24} color={accent} strokeWidth={1.8} />
              {String(label)}
            </div>
          );
        })}
      </div>

      <Interactive.Div
        name="Taskbar settings panel"
        style={{
          position: "absolute",
          right: 120,
          top: 165,
          width: 845,
          height: 590,
          padding: 28,
          borderRadius: 22,
          color: "#f3f2f7",
          backgroundColor: "rgba(22,23,31,0.97)",
          border: "1px solid rgba(255,255,255,0.1)",
          boxShadow:
            "0 28px 80px rgba(0,0,0,0.48), inset 0 1px 0 rgba(255,255,255,0.05)",
          opacity: interpolate(frame, [0.15 * fps, 0.75 * fps], [0, 1], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          }),
          translate: interpolate(
            frame,
            [0.1 * fps, 0.9 * fps],
            ["80px 0px", "0px 0px"],
            {
              extrapolateLeft: "clamp",
              extrapolateRight: "clamp",
              easing: Easing.spring({ damping: 190 }),
            },
          ),
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
          }}
        >
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 13,
              fontSize: 28,
              fontWeight: 720,
            }}
          >
            <Settings2 size={27} color={accent} strokeWidth={1.8} />
            Taskbar
          </div>
          <div style={{ color: "#807e8a", fontSize: 17 }}>
            Applied instantly
          </div>
        </div>
        <div
          style={{
            height: 1,
            backgroundColor: "rgba(255,255,255,0.08)",
            margin: "25px 0",
          }}
        />

        <div
          style={{
            display: "grid",
            gridTemplateColumns: "230px 1fr",
            alignItems: "center",
            marginBottom: 25,
          }}
        >
          <div>
            <div style={{ fontSize: 21, fontWeight: 660 }}>Alignment</div>
            <div style={{ color: "#85828e", fontSize: 16, marginTop: 4 }}>
              Position taskbar apps
            </div>
          </div>
          <div
            style={{
              display: "flex",
              gap: 3,
              padding: 4,
              borderRadius: 13,
              backgroundColor: "rgba(255,255,255,0.04)",
              border: "1px solid rgba(255,255,255,0.07)",
            }}
          >
            <SegmentedOption
              active={leftAligned}
              accent={accent}
              label="Left"
              icon={<AlignLeft size={20} />}
            />
            <SegmentedOption
              active={!leftAligned}
              accent={accent}
              label="Center"
              icon={<AlignCenter size={20} />}
            />
          </div>
        </div>

        <div
          style={{
            display: "grid",
            gridTemplateColumns: "230px 1fr",
            alignItems: "center",
            marginBottom: 25,
          }}
        >
          <div>
            <div style={{ fontSize: 21, fontWeight: 660 }}>Icon density</div>
            <div style={{ color: "#85828e", fontSize: 16, marginTop: 4 }}>
              Scale taskbar icons
            </div>
          </div>
          <div
            style={{
              display: "flex",
              gap: 3,
              padding: 4,
              borderRadius: 13,
              backgroundColor: "rgba(255,255,255,0.04)",
              border: "1px solid rgba(255,255,255,0.07)",
            }}
          >
            <SegmentedOption
              active={false}
              accent={accent}
              label="Compact"
              icon={<span>•</span>}
            />
            <SegmentedOption
              active={true}
              accent={accent}
              label="Default"
              icon={<Check size={19} />}
            />
            <SegmentedOption
              active={false}
              accent={accent}
              label="When full"
              icon={<span>••</span>}
            />
          </div>
        </div>

        <div
          style={{
            display: "grid",
            gridTemplateColumns: "230px 1fr",
            alignItems: "center",
            marginBottom: 25,
          }}
        >
          <div>
            <div style={{ fontSize: 21, fontWeight: 660 }}>Start icon</div>
            <div style={{ color: "#85828e", fontSize: 16, marginTop: 4 }}>
              System, Gem, or custom
            </div>
          </div>
          <div style={{ display: "flex", gap: 12 }}>
            <div
              style={{
                width: 58,
                height: 58,
                borderRadius: 12,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                backgroundColor: `${accent}18`,
                border: `1px solid ${accent}66`,
              }}
            >
              <Img
                src={staticFile("prism-icon.png")}
                style={{ width: 38, height: 38, borderRadius: 9 }}
              />
            </div>
            <div
              style={{
                width: 58,
                height: 58,
                borderRadius: 12,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                color: "#817e8a",
                backgroundColor: "rgba(255,255,255,0.04)",
                border: "1px solid rgba(255,255,255,0.08)",
              }}
            >
              <Gem size={29} />
            </div>
          </div>
        </div>

        <div
          style={{
            display: "grid",
            gridTemplateColumns: "230px 1fr",
            alignItems: "center",
          }}
        >
          <div>
            <div style={{ fontSize: 21, fontWeight: 660 }}>Auto-hide</div>
            <div style={{ color: "#85828e", fontSize: 16, marginTop: 4 }}>
              Maximize screen space
            </div>
          </div>
          <div
            style={{
              width: 58,
              height: 32,
              borderRadius: 16,
              padding: 3,
              backgroundColor: "rgba(255,255,255,0.09)",
              border: "1px solid rgba(255,255,255,0.09)",
            }}
          >
            <div
              style={{
                width: 24,
                height: 24,
                borderRadius: 12,
                backgroundColor: "#918e99",
              }}
            />
          </div>
        </div>
      </Interactive.Div>

      <Interactive.Div
        name="Windows taskbar preview"
        style={{
          position: "absolute",
          left: 108,
          right: 108,
          bottom: 55,
          height: 92,
          borderRadius: 18,
          backgroundColor: "rgba(20,21,28,0.98)",
          border: "1px solid rgba(255,255,255,0.1)",
          boxShadow: "0 16px 46px rgba(0,0,0,0.44)",
        }}
      >
        <Interactive.Div
          name="Taskbar apps"
          style={{
            position: "absolute",
            top: 14,
            left: "50%",
            display: "flex",
            alignItems: "center",
            gap: 13,
            translate: interpolate(
              frame,
              [2.35 * fps, 3.55 * fps],
              ["-163px 0px", "-838px 0px"],
              {
                extrapolateLeft: "clamp",
                extrapolateRight: "clamp",
                easing: Easing.spring({ damping: 170 }),
              },
            ),
          }}
        >
          <Img
            src={staticFile("prism-icon.png")}
            style={{ width: 58, height: 58, borderRadius: 13 }}
          />
          {["#49a8f2", "#e7be45", "#46c98a", "#e36f8b"].map((color) => (
            <div
              key={color}
              style={{
                width: 58,
                height: 58,
                borderRadius: 13,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                color,
                backgroundColor: "rgba(255,255,255,0.05)",
              }}
            >
              <AppWindow size={29} strokeWidth={1.7} />
            </div>
          ))}
        </Interactive.Div>
        <div
          style={{
            position: "absolute",
            right: 24,
            top: 34,
            color: "#a5a2ad",
            fontSize: 17,
          }}
        >
          5:42 PM&nbsp;&nbsp; 8/17/2026
        </div>
      </Interactive.Div>
    </SceneBackdrop>
  );
};
