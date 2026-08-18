import {
  AppWindow,
  Calculator,
  FileText,
  FolderOpen,
  Image,
  Search,
  Settings2,
} from "lucide-react";
import { Interactive } from "remotion";

export type ResultKind = "app" | "file" | "folder" | "calc" | "image";

export type ResultRow = {
  kind: ResultKind;
  title: string;
  subtitle: string;
  action?: string;
};

const ResultIcon: React.FC<{ kind: ResultKind; accent: string }> = ({
  kind,
  accent,
}) => {
  const iconProps = { size: 25, strokeWidth: 1.8 };
  const Icon =
    kind === "app"
      ? AppWindow
      : kind === "folder"
        ? FolderOpen
        : kind === "calc"
          ? Calculator
          : kind === "image"
            ? Image
            : FileText;

  return (
    <div
      style={{
        width: 48,
        height: 48,
        borderRadius: 12,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: accent,
        backgroundColor: "rgba(143,124,245,0.12)",
        border: "1px solid rgba(255,255,255,0.07)",
        flex: "0 0 auto",
      }}
    >
      <Icon {...iconProps} />
    </div>
  );
};

export const PrismWindow: React.FC<{
  accent: string;
  query: string;
  results: ResultRow[];
  selected?: number;
  footer?: string;
}> = ({
  accent,
  query,
  results,
  selected = 0,
  footer = "↑↓  Navigate     Enter  Open",
}) => {
  return (
    <Interactive.Div
      name="Prism product window"
      style={{
        width: 1060,
        minHeight: 596,
        borderRadius: 24,
        padding: 20,
        backgroundColor: "rgba(22,23,31,0.96)",
        border: "1px solid rgba(255,255,255,0.1)",
        boxShadow:
          "0 30px 90px rgba(0,0,0,0.52), inset 0 1px 0 rgba(255,255,255,0.05)",
        overflow: "hidden",
      }}
    >
      <div
        style={{
          height: 80,
          borderRadius: 16,
          display: "flex",
          alignItems: "center",
          gap: 18,
          padding: "0 22px",
          backgroundColor: "rgba(255,255,255,0.045)",
          border: "1px solid rgba(255,255,255,0.07)",
          boxShadow: query ? `0 0 0 2px ${accent}55` : "none",
        }}
      >
        <Search
          size={29}
          strokeWidth={1.8}
          color={query ? accent : "#8b8b98"}
        />
        <div
          style={{
            flex: 1,
            color: query ? "#f8f7fb" : "#858490",
            fontSize: 29,
            fontWeight: 500,
            letterSpacing: 0,
          }}
        >
          {query || "Search apps, files, folders, or calculate..."}
          {query ? (
            <span style={{ color: accent, marginLeft: 3 }}>|</span>
          ) : null}
        </div>
        <div
          style={{
            height: 40,
            minWidth: 72,
            padding: "0 12px",
            borderRadius: 9,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: "#aaa8b3",
            fontSize: 18,
            fontWeight: 650,
            backgroundColor: "rgba(255,255,255,0.05)",
            border: "1px solid rgba(255,255,255,0.08)",
          }}
        >
          ESC
        </div>
        <Settings2 size={26} strokeWidth={1.8} color="#8d8b98" />
      </div>

      <div style={{ padding: "18px 4px 8px" }}>
        <div
          style={{
            padding: "0 16px 10px",
            color: "#7d7a88",
            fontSize: 18,
            fontWeight: 700,
            letterSpacing: 0,
            textTransform: "uppercase",
          }}
        >
          Best matches
        </div>
        {results.map((result, index) => (
          <div
            key={`${result.title}-${result.subtitle}`}
            style={{
              height: 78,
              borderRadius: 14,
              display: "flex",
              alignItems: "center",
              gap: 16,
              padding: "0 16px",
              marginBottom: 4,
              backgroundColor:
                index === selected ? "rgba(255,255,255,0.075)" : "transparent",
              border:
                index === selected
                  ? `1px solid ${accent}55`
                  : "1px solid transparent",
            }}
          >
            <ResultIcon kind={result.kind} accent={accent} />
            <div style={{ minWidth: 0, flex: 1 }}>
              <div
                style={{
                  color: "#f3f2f7",
                  fontSize: 24,
                  fontWeight: 650,
                  letterSpacing: 0,
                }}
              >
                {result.title}
              </div>
              <div
                style={{
                  color: "#8f8d99",
                  fontSize: 17,
                  marginTop: 3,
                  letterSpacing: 0,
                }}
              >
                {result.subtitle}
              </div>
            </div>
            {result.action ? (
              <div
                style={{
                  color: index === selected ? accent : "#777581",
                  fontSize: 17,
                  fontWeight: 650,
                }}
              >
                {result.action}
              </div>
            ) : null}
          </div>
        ))}
      </div>

      <div
        style={{
          height: 47,
          margin: "4px -20px -20px",
          padding: "0 25px",
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          color: "#767481",
          fontSize: 16,
          borderTop: "1px solid rgba(255,255,255,0.07)",
          backgroundColor: "rgba(5,6,9,0.2)",
        }}
      >
        <span>{footer}</span>
        <span>Prism 0.9.8</span>
      </div>
    </Interactive.Div>
  );
};
