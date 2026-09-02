import {Video} from "@remotion/media";
import {
  AbsoluteFill,
  Easing,
  Img,
  interpolate,
  staticFile,
  useCurrentFrame,
} from "remotion";
import {
  COLORS,
  FeaturePill,
  Keycap,
  Logo,
  ProductImage,
  Reveal,
  SceneFrame,
  reveal,
} from "./components";
import {ASSETS} from "./timeline.js";

const titleStyle = {
  margin: 0,
  color: COLORS.paper,
  fontSize: 78,
  lineHeight: 0.98,
  fontWeight: 720,
  letterSpacing: 0,
} as const;

const bodyStyle = {
  margin: 0,
  color: COLORS.muted,
  fontSize: 27,
  lineHeight: 1.32,
  fontWeight: 500,
  letterSpacing: 0,
} as const;

export const IntroScene = () => {
  const frame = useCurrentFrame();
  const imageProgress = reveal(frame, 14, 34);
  const words = ["Find.", "Open.", "Control."];

  return (
    <SceneFrame label="Windows, refined">
      <div style={{position: "absolute", left: 108, top: 133, width: 760}}>
        <Reveal delay={4} style={{display: "flex", alignItems: "center", gap: 20, marginBottom: 64}}>
          <Logo size={86} />
          <div style={{fontSize: 62, fontWeight: 740, letterSpacing: 0}}>Prism</div>
        </Reveal>
        <div>
          {words.map((word, index) => {
            const progress = reveal(frame, 10 + index * 7, 25);
            return (
              <div
                key={word}
                style={{
                  color: index === 2 ? COLORS.violet : COLORS.paper,
                  fontSize: 118,
                  lineHeight: 0.89,
                  fontWeight: 760,
                  letterSpacing: 0,
                  opacity: progress,
                  transform: `translateX(${(1 - progress) * -48}px)`,
                }}
              >
                {word}
              </div>
            );
          })}
        </div>
        <Reveal delay={38} style={{marginTop: 48, display: "flex", alignItems: "center", gap: 12}}>
          <Keycap>Win</Keycap>
          <span style={{fontSize: 21, color: COLORS.muted}}>Your Windows workflow, one key away.</span>
        </Reveal>
      </div>
      <div
        style={{
          position: "absolute",
          top: 160,
          right: 160,
          width: 560,
          height: 620,
          opacity: imageProgress,
          transform: `translateX(${(1 - imageProgress) * 76}px) rotate(${(1 - imageProgress) * 1.2}deg)`,
          transformOrigin: "center",
        }}
      >
        <ProductImage src={ASSETS.launcher} width={560} />
      </div>
      <div
        style={{
          position: "absolute",
          left: 108,
          bottom: 74,
          width: 520,
          height: 4,
          backgroundColor: COLORS.panelRaised,
        }}
      >
        <div
          style={{
            height: "100%",
            width: `${interpolate(frame, [0, 72], [0, 100], {extrapolateRight: "clamp"})}%`,
            backgroundColor: COLORS.violet,
          }}
        />
      </div>
    </SceneFrame>
  );
};

export const LauncherScene = () => {
  const frame = useCurrentFrame();
  const scan = interpolate(frame, [18, 110], [0, 1], {
    easing: Easing.bezier(0.45, 0, 0.55, 1),
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  return (
    <SceneFrame label="Local search">
      <div style={{position: "absolute", left: 108, top: 170, width: 620}}>
        <Reveal delay={2}>
          <h2 style={titleStyle}>Everything.<br />One place.</h2>
        </Reveal>
        <Reveal delay={12} style={{marginTop: 34}}>
          <p style={bodyStyle}>Search apps, files, and folders without sending the query to a hosted service.</p>
        </Reveal>
        <Reveal delay={24} style={{display: "flex", gap: 12, marginTop: 42, flexWrap: "wrap"}}>
          <FeaturePill>Applications</FeaturePill>
          <FeaturePill accent={COLORS.mint}>Files</FeaturePill>
          <FeaturePill accent={COLORS.amber}>Folders</FeaturePill>
        </Reveal>
      </div>
      <div style={{position: "absolute", right: 140, top: 153}}>
        <ProductImage src={ASSETS.launcher} width={675} />
        <div
          style={{
            position: "absolute",
            left: 26,
            right: 26,
            top: 23 + scan * 470,
            height: 2,
            backgroundColor: COLORS.violet,
            opacity: 0.28,
          }}
        />
      </div>
      <Reveal delay={38} style={{position: "absolute", left: 108, bottom: 100, display: "flex", alignItems: "center", gap: 14}}>
        <Keycap>Enter</Keycap>
        <span style={{fontSize: 20, color: COLORS.violet}}>Open the selected result</span>
      </Reveal>
    </SceneFrame>
  );
};

export const DemoScene = () => {
  const frame = useCurrentFrame();
  const scale = interpolate(frame, [0, 183], [1.02, 1.13], {
    easing: Easing.inOut(Easing.quad),
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const titleProgress = reveal(frame, 4, 22);

  return (
    <AbsoluteFill
      style={{
        backgroundColor: COLORS.canvas,
        overflow: "hidden",
        fontFamily: '\"Segoe UI Variable\", \"Segoe UI\", Arial, sans-serif',
      }}
    >
      <Video
        src={staticFile(ASSETS.demo)}
        muted
        playbackRate={2}
        objectFit="cover"
        style={{
          position: "absolute",
          width: "100%",
          height: "100%",
          transform: `scale(${scale})`,
        }}
      />
      <div style={{position: "absolute", inset: 0, backgroundColor: "rgba(7, 8, 13, 0.18)"}} />
      <div style={{position: "absolute", left: 0, top: 0, bottom: 0, width: 8, backgroundColor: COLORS.mint}} />
      <div
        style={{
          position: "absolute",
          left: 94,
          top: 82,
          width: 670,
          padding: "30px 36px 34px",
          backgroundColor: "rgba(9, 10, 15, 0.92)",
          border: `1px solid ${COLORS.line}`,
          borderRadius: 8,
          opacity: titleProgress,
          transform: `translateY(${(1 - titleProgress) * 30}px)`,
        }}
      >
        <div style={{color: COLORS.mint, fontSize: 17, fontWeight: 700, textTransform: "uppercase"}}>Real workflow</div>
        <h2 style={{...titleStyle, marginTop: 13, fontSize: 62}}>Control Windows directly.</h2>
        <p style={{...bodyStyle, marginTop: 20, fontSize: 23}}>Taskbar, appearance, shortcuts, and launcher behavior in one surface.</p>
      </div>
      <div style={{position: "absolute", right: 90, bottom: 66, display: "flex", gap: 12}}>
        <FeaturePill accent={COLORS.mint}>Actual Prism recording</FeaturePill>
        <FeaturePill>No StartAllBack required</FeaturePill>
      </div>
    </AbsoluteFill>
  );
};

export const QuickAccessScene = () => (
  <SceneFrame label="Quick access" labelColor={COLORS.amber}>
    <div style={{position: "absolute", left: 108, top: 188, width: 590}}>
      <Reveal delay={2}>
        <h2 style={titleStyle}>Your folders.<br />Already there.</h2>
      </Reveal>
      <Reveal delay={14} style={{marginTop: 34}}>
        <p style={bodyStyle}>Keep up to six frequent locations beside your launcher results.</p>
      </Reveal>
      <Reveal delay={28} style={{marginTop: 42, display: "flex", gap: 12}}>
        <FeaturePill accent={COLORS.amber}>Pin folders</FeaturePill>
        <FeaturePill accent={COLORS.mint}>Open in one step</FeaturePill>
      </Reveal>
    </div>
    <div style={{position: "absolute", right: 115, top: 116}}>
      <ProductImage src={ASSETS.quickAccess} width={790} />
    </div>
  </SceneFrame>
);

export const SettingsScene = () => (
  <SceneFrame label="Built around you" labelColor={COLORS.mint}>
    <div style={{position: "absolute", left: 108, top: 183, width: 600}}>
      <Reveal delay={2}>
        <h2 style={titleStyle}>Make it yours.</h2>
      </Reveal>
      <Reveal delay={12} style={{marginTop: 34}}>
        <p style={bodyStyle}>Choose the theme, accent, width, material, and view scale that fit your desktop.</p>
      </Reveal>
      <Reveal delay={26} style={{marginTop: 42, display: "flex", gap: 12, flexWrap: "wrap"}}>
        <FeaturePill>Dark or light</FeaturePill>
        <FeaturePill accent={COLORS.mint}>Five accents</FeaturePill>
        <FeaturePill accent={COLORS.amber}>Acrylic, mica, solid</FeaturePill>
      </Reveal>
    </div>
    <div style={{position: "absolute", right: 112, top: 114}}>
      <ProductImage src={ASSETS.settings} width={800} />
    </div>
  </SceneFrame>
);

export const OutroScene = () => {
  const frame = useCurrentFrame();
  const logoProgress = reveal(frame, 2, 28);
  const rule = interpolate(frame, [10, 54], [0, 1], {
    easing: Easing.bezier(0.16, 1, 0.3, 1),
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  return (
    <SceneFrame label="Ready when you are">
      <div style={{position: "absolute", left: 184, top: 260, opacity: logoProgress, transform: `scale(${0.9 + logoProgress * 0.1})`}}>
        <Logo size={270} />
      </div>
      <div style={{position: "absolute", left: 590, top: 250, width: 1050}}>
        <Reveal delay={8}>
          <h2 style={{...titleStyle, fontSize: 112}}>Meet Prism.</h2>
        </Reveal>
        <Reveal delay={17} style={{marginTop: 28}}>
          <p style={{...bodyStyle, fontSize: 32}}>A keyboard-first command palette and Windows taskbar companion.</p>
        </Reveal>
        <Reveal delay={29} style={{display: "flex", alignItems: "center", gap: 14, marginTop: 43}}>
          <div style={{padding: "17px 25px", borderRadius: 6, backgroundColor: COLORS.violet, color: COLORS.canvas, fontSize: 20, fontWeight: 750}}>
            Free for Windows 10 &amp; 11
          </div>
          <div style={{padding: "16px 24px", borderRadius: 6, border: `1px solid ${COLORS.line}`, color: COLORS.paper, fontSize: 20, fontWeight: 650}}>
            github.com/Hi9841/Prism
          </div>
        </Reveal>
        <div style={{marginTop: 42, width: 770, height: 2, backgroundColor: COLORS.line}}>
          <div style={{height: "100%", width: `${rule * 100}%`, backgroundColor: COLORS.violet}} />
        </div>
        <Reveal delay={42} style={{marginTop: 18, color: COLORS.faint, fontSize: 17}}>
          Windows x64 | MIT licensed | Current-user install
        </Reveal>
      </div>
    </SceneFrame>
  );
};
