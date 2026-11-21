import { describe, expect, test } from "bun:test";
import { diagnosticsFilename, downloadDiagnostics, type FetchLike } from "./diagnosticsDownload";

describe("diagnosticsFilename", () => {
  test("takes the server's attachment name", () => {
    expect(diagnosticsFilename('attachment; filename="mestro-diagnostics-a8724d0c4171.json"')).toBe(
      "mestro-diagnostics-a8724d0c4171.json",
    );
  });

  test("falls back to a fixed name without a usable header", () => {
    expect(diagnosticsFilename(null)).toBe("mestro-diagnostics.json");
    expect(diagnosticsFilename("attachment")).toBe("mestro-diagnostics.json");
    expect(diagnosticsFilename('attachment; filename="../../etc/passwd"')).toBe(
      "mestro-diagnostics.json",
    );
  });
});

describe("downloadDiagnostics", () => {
  test("returns the file name and the body of a 200", async () => {
    const fetchLike: FetchLike = async (url) => {
      expect(url).toBe("/diagnostics");
      return new Response('{"report":"mestro-diagnostics"}', {
        status: 200,
        headers: { "content-disposition": 'attachment; filename="mestro-diagnostics-x1.json"' },
      });
    };
    const file = await downloadDiagnostics(fetchLike);
    expect(file.name).toBe("mestro-diagnostics-x1.json");
    expect(await file.blob.text()).toBe('{"report":"mestro-diagnostics"}');
  });

  test("an unpaired 401 is a readable error", async () => {
    const fetchLike: FetchLike = async () => new Response("pairing required", { status: 401 });
    await expect(downloadDiagnostics(fetchLike)).rejects.toThrow("pairing required");
  });

  test("a network failure is an error too", async () => {
    const fetchLike: FetchLike = async () => {
      throw new TypeError("Failed to fetch");
    };
    await expect(downloadDiagnostics(fetchLike)).rejects.toThrow("Failed to fetch");
  });
});
