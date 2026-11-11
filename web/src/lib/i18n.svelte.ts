// tiny i18n for the wizard: two locales, no framework. The kiosk
// UI uses Slint's @tr catalogs — this mirrors the strings for the
// browser. Default is the browser language, switchable in the
// footer.

export type Locale = "de" | "en";

const strings = {
  de: {
    "brand.sub": "mestro einrichten",
    "app.loading": "Wird geladen …",
    "steps.server": "Music Assistant",
    "steps.login": "Zugang",
    "steps.player": "Player",

    "intro.title": "mestro einrichten",
    "intro.fresh":
      "Dieses Gerät hat noch keine Verbindung zu Music Assistant. Richte es von hier aus ein — die Werte landen direkt auf dem Gerät.",
    "intro.configured":
      "Dieser Kiosk ist bereits eingerichtet. Du kannst die Einrichtung neu starten, z. B. für einen anderen Server oder Player.",
    "intro.start": "Los geht’s →",
    "intro.restart": "Neu einrichten →",

    "url.title": "Music Assistant",
    "url.lead": "Adresse deines Music-Assistant-Servers.",
    "url.label": "Server-URL",
    "url.probing": "wird geprüft…",
    "url.ok": "erreichbar · {version} · Schema {schema}",
    "url.next": "Weiter →",
    "url.checking": "Prüfe…",

    "login.title": "Bei Music Assistant anmelden",
    "login.lead":
      "Deine MA-Zugangsdaten. mestro erstellt daraus einen eigenen Zugang — das Passwort wird nicht gespeichert.",
    "login.user": "Benutzername",
    "login.pass": "Passwort",
    "login.manualLink": "Token im Music Assistant erstellen",
    "login.manualForm": "Token manuell eingeben",
    "login.manualHide": "Token-Eingabe verbergen",
    "login.manualHint": "Einstellungen → Profile",
    "login.token": "Token",
    "login.tokenPlaceholder": "aus MA → Einstellungen → Profile",
    "login.saveToken": "Token speichern",
    "login.submit": "Anmelden →",

    "player.title": "Player wählen",
    "player.lead": "Welchen Player soll dieser Bildschirm steuern?",
    "player.loading": "werden geladen…",
    "player.empty": "Keine Player gefunden.",

    "done.title": "Einrichtung abgeschlossen",
    "done.waiting": "mestro verbindet sich mit Music Assistant …",
    "done.ready": "Der Kiosk ist eingerichtet und verbunden.",
    "done.ok": "Music Assistant · verbunden",
    "done.pending": "Music Assistant · verbinde …",
    "done.close": "Fenster kann geschlossen werden",

    "common.back": "← Zurück",
    "common.errorConnect":
      "Kiosk nicht erreichbar oder Pairing ungültig — bitte die URL aus dem Kiosk-Display öffnen.",
  },
  en: {
    "brand.sub": "mestro setup",
    "app.loading": "Loading …",
    "steps.server": "Music Assistant",
    "steps.login": "Sign in",
    "steps.player": "Player",

    "intro.title": "Set up mestro",
    "intro.fresh":
      "This device is not connected to Music Assistant yet. Set it up from here — the values are written to the device.",
    "intro.configured":
      "This kiosk is already set up. You can start over, e.g. for a different server or player.",
    "intro.start": "Get started →",
    "intro.restart": "Start over →",

    "url.title": "Music Assistant",
    "url.lead": "Address of your Music Assistant server.",
    "url.label": "Server URL",
    "url.probing": "checking…",
    "url.ok": "reachable · {version} · schema {schema}",
    "url.next": "Next →",
    "url.checking": "Checking…",

    "login.title": "Sign in to Music Assistant",
    "login.lead":
      "Your MA credentials. mestro creates its own access from them — the password is not stored.",
    "login.user": "Username",
    "login.pass": "Password",
    "login.manualLink": "Create a token in Music Assistant",
    "login.manualForm": "Enter token manually",
    "login.manualHide": "Hide token entry",
    "login.manualHint": "Settings → Profile",
    "login.token": "Token",
    "login.tokenPlaceholder": "from MA → Settings → Profile",
    "login.saveToken": "Save token",
    "login.submit": "Sign in →",

    "player.title": "Choose a player",
    "player.lead": "Which player should this screen control?",
    "player.loading": "loading…",
    "player.empty": "No players found.",

    "done.title": "Setup complete",
    "done.waiting": "mestro is connecting to Music Assistant …",
    "done.ready": "The kiosk is set up and connected.",
    "done.ok": "Music Assistant · connected",
    "done.pending": "Music Assistant · connecting …",
    "done.close": "You can close this window",

    "common.back": "← Back",
    "common.errorConnect":
      "Kiosk unreachable or pairing invalid — please open the URL shown on the kiosk display.",
  },
} as const;

export type Key = keyof (typeof strings)["de"];

function detectLocale(): Locale {
  return navigator.language.toLowerCase().startsWith("de") ? "de" : "en";
}

// module-level runes: every component imports these directly, no
// prop drilling (works in *.svelte.ts, runes mode). State lives in
// one object — Svelte forbids reassigning an exported $state.
const state = $state({ locale: detectLocale() });

export function getLocale(): Locale {
  return state.locale;
}

export function setLocale(next: Locale) {
  state.locale = next;
}

export function toggleLocale() {
  state.locale = state.locale === "de" ? "en" : "de";
}

export function t(key: Key, vars?: Record<string, string>): string {
  let text: string = strings[state.locale][key] ?? strings.en[key] ?? key;
  for (const [name, value] of Object.entries(vars ?? {})) {
    text = text.replace(`{${name}}`, value);
  }
  return text;
}
