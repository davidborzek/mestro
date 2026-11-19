// the factory-reset sheet's typed confirmation: the word is
// locale-fixed (de ZURÜCKSETZEN, en RESET) and matched
// case-insensitively after trimming
export function confirmMatches(input: string, word: string): boolean {
  return input.trim().toLowerCase() === word.trim().toLowerCase();
}
