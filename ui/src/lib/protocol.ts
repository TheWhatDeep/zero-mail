// Custom protocol URLs. Core returns `mail-body://localhost/<id>?images=0`; Windows (and
// Android) webviews reach custom protocols as `http://<scheme>.localhost/...` instead.

export function usesHttpScheme(userAgent: string): boolean {
  return /Windows|Android/.test(userAgent);
}

export function platformUrl(canonical: string, httpScheme: boolean): string {
  const m = /^([a-z][a-z0-9-]*):\/\/localhost\/(.*)$/.exec(canonical);
  if (!m || !httpScheme) return canonical;
  return `http://${m[1]}.localhost/${m[2]}`;
}
