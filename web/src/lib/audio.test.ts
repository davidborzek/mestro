import { describe, expect, test } from "bun:test";
import { currentOutput, outputLabel, switchOutput, type AudioSend } from "./audio";
import type { AudioOutput } from "./protocol";

const OUTPUTS: AudioOutput[] = [
  { id: "hw:CARD=Headphones", name: "bcm2835 Headphones", current: true },
  { id: "hw:CARD=S3", name: "Topping S3", current: false },
];

describe("currentOutput", () => {
  test("is the entry the device marks as in use", () => {
    expect(currentOutput(OUTPUTS)?.id).toBe("hw:CARD=Headphones");
    expect(currentOutput([])).toBeNull();
    expect(currentOutput([{ ...OUTPUTS[1] }])).toBeNull();
  });
});

describe("outputLabel", () => {
  test("names the current device, else the placeholder", () => {
    expect(outputLabel(OUTPUTS, "none")).toBe("bcm2835 Headphones");
    expect(outputLabel([OUTPUTS[1]], "none")).toBe("none");
  });
});

describe("switchOutput", () => {
  test("sends set and returns the refreshed list with the new device current", async () => {
    const sent: unknown[] = [];
    const send: AudioSend = async (request) => {
      sent.push(request);
      if (request.action === "set") return { output: "hw:CARD=S3" };
      return {
        outputs: [
          { ...OUTPUTS[0], current: false },
          { ...OUTPUTS[1], current: true },
        ],
        current: "hw:CARD=S3",
      };
    };
    const result = await switchOutput(send, "hw:CARD=S3");
    expect(sent).toEqual([
      { op: "audio", action: "set", output: "hw:CARD=S3" },
      { op: "audio", action: "outputs" },
    ]);
    expect(result.error).toBe("");
    expect(currentOutput(result.outputs)?.id).toBe("hw:CARD=S3");
  });

  test("a failed switch keeps the error text and re-reads the list", async () => {
    const send: AudioSend = async (request) => {
      if (request.action === "set") {
        throw new Error("squeezelite could not start on hw:CARD=S3 — back on the previous output");
      }
      return { outputs: OUTPUTS, current: "hw:CARD=Headphones" };
    };
    const result = await switchOutput(send, "hw:CARD=S3");
    expect(result.error).toContain("hw:CARD=S3");
    expect(currentOutput(result.outputs)?.id).toBe("hw:CARD=Headphones");
  });
});
