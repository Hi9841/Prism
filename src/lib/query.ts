export type Phase1Kind = "recent" | "window" | "app" | "action";

export interface RecentCommand {
  id: string;
  title: string;
}

export interface Phase1Hit {
  id: string;
  kind: Phase1Kind;
  title: string;
  subtitle: string;
  score: number;
  appId?: string;
  path?: string;
  hwnd?: number;
  actionId?: string;
  uri?: string;
  iconKey?: string;
  icon?: string;
  source?: string;
}

export interface Phase1Response {
  query: string;
  recents: Phase1Hit[];
  windows: Phase1Hit[];
  apps: Phase1Hit[];
  actions: Phase1Hit[];
  intentActionId?: string;
}

export const EMPTY_PHASE1: Phase1Response = {
  query: "",
  recents: [],
  windows: [],
  apps: [],
  actions: [],
};

export function phase1MatchesQuery(phase1: Phase1Response | null | undefined, query: string): boolean {
  if (!phase1) return false;
  return phase1.query.trim().toLowerCase() === query.trim().toLowerCase();
}
