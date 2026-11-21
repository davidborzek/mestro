// the hand-built settings sections the served schema does not
// carry: the MA connection (listed first) and the device sections
// (listed after the schema's own) — all render fixed pages, not
// schema fields. DOM-free so bun test covers the visibility rules.

export const MA_SECTION_ID = "connection";
export const AUDIO_SECTION_ID = "audio";
export const INFO_SECTION_ID = "info";

// the device sections in list order
export const DEVICE_SECTION_IDS = [AUDIO_SECTION_ID, INFO_SECTION_ID] as const;
export type DeviceSectionId = (typeof DEVICE_SECTION_IDS)[number];

// the hello capabilities that decide which device sections exist
export interface DeviceSectionFacts {
  audioOutput: boolean;
}

function available(id: DeviceSectionId, facts: DeviceSectionFacts): boolean {
  switch (id) {
    case AUDIO_SECTION_ID:
      return facts.audioOutput;
    case INFO_SECTION_ID:
      return true;
  }
}

export function availableDeviceSections(facts: DeviceSectionFacts): DeviceSectionId[] {
  return DEVICE_SECTION_IDS.filter((id) => available(id, facts));
}

// the route's section when it is a device section this build has;
// anything else (unknown, or unavailable here) is null and falls
// back like an unknown section does
export function openDeviceSection(
  id: string | null,
  facts: DeviceSectionFacts,
): DeviceSectionId | null {
  return availableDeviceSections(facts).find((candidate) => candidate === id) ?? null;
}
