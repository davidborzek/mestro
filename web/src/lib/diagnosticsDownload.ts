// the "Download diagnostics" button's logic: fetch the report from
// the player and hand back a file name and the body. DOM-free — the
// component turns the blob into a download.

export type FetchLike = (url: string) => Promise<Response>;

const FALLBACK_NAME = "mestro-diagnostics.json";

// the attachment name the player suggests, only when it is a plain
// file name (a header must never pick a path)
export function diagnosticsFilename(contentDisposition: string | null): string {
  const match = contentDisposition?.match(/filename="([A-Za-z0-9._-]+)"/);
  const name = match?.[1];
  return name && !name.startsWith(".") ? name : FALLBACK_NAME;
}

export async function downloadDiagnostics(
  fetchLike: FetchLike = (url) => fetch(url),
): Promise<{ name: string; blob: Blob }> {
  const response = await fetchLike("/diagnostics");
  if (!response.ok) {
    const text = (await response.text()).trim();
    throw new Error(text || `HTTP ${response.status}`);
  }
  return {
    name: diagnosticsFilename(response.headers.get("content-disposition")),
    blob: await response.blob(),
  };
}
