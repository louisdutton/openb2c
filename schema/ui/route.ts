export type ParsedHash = {
  path: string;
  params: URLSearchParams;
};

export function parseHash(hash: string): ParsedHash {
  const raw = hash.slice(1) || "/";
  const [path, query = ""] = raw.split("?");
  return { path: path || "/", params: new URLSearchParams(query) };
}

export function safeReturnTo(value: string | null): string {
  if (!value || !value.startsWith("/") || value.startsWith("//")) return "";
  return value.split("#")[0];
}

export function collectionRoutePath(entity?: string | null): string {
  if (!entity) return "/";
  return `/${pluralRouteSegment(entity)}`;
}

function pluralRouteSegment(entity: string): string {
  if (/[^aeiou]y$/i.test(entity)) return `${entity.slice(0, -1)}ies`;
  if (entity.endsWith("s")) return `${entity}es`;
  return `${entity}s`;
}
