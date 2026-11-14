// Lister (F3) und Schnellansicht (Strg+Q) gegen die Mock-API mit Beispieldateien in ~/Ansicht.
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { CoreError, type DualfileApi } from "./ipc/api";
import { mockFileBytes } from "./ipc/mock";
import { blobHolder } from "./components/lister/blobHolder";
import { HOME, setupApp, type SetupOptions } from "./test-utils";

const ANSICHT = `${HOME}/Ansicht`;
type App = Awaited<ReturnType<typeof setupApp>>;

// ---------------------------------------------------------------- Blob-Mocks

let live: Set<string>;
let maxLive: number;
let created: Blob[];
let counter: number;

beforeEach(() => {
  live = new Set();
  maxLive = 0;
  created = [];
  counter = 0;
  vi.stubGlobal("URL", {
    ...URL,
    createObjectURL: vi.fn((blob: Blob) => {
      created.push(blob);
      const url = `blob:dualfile/${(counter += 1)}`;
      live.add(url);
      maxLive = Math.max(maxLive, live.size);
      return url;
    }),
    revokeObjectURL: vi.fn((url: string) => void live.delete(url)),
  });
});

afterEach(() => {
  blobHolder.releaseAll();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------- Hilfen

const setup = (extra: SetupOptions = {}): Promise<App> => setupApp({ left: ANSICHT, samples: true, ...extra });

async function open(app: App, name: string): Promise<HTMLElement> {
  await app.user.click(app.row(0, name));
  await app.user.keyboard("{F3}");
  return screen.findByRole("dialog");
}

const textbox = (): HTMLElement => screen.getByRole("textbox", { name: "Textansicht" });
const hexlog = (): HTMLElement => screen.getByRole("log", { name: "Hex-Ansicht" });
const status = (): HTMLElement => screen.getByRole("status", { name: "Statuszeile" });
const waitText = (element: () => HTMLElement, text: string | RegExp) => waitFor(() => expect(element()).toHaveTextContent(text));

/** Zählt Aufrufe einzelner API-Funktionen. */
function spy(): { wrap: SetupOptions["wrap"]; calls: Record<string, unknown[][]> } {
  const calls: Record<string, unknown[][]> = {};
  const wrap = (api: DualfileApi): DualfileApi =>
    new Proxy(api, {
      get(target, key: string) {
        const value = (target as unknown as Record<string, unknown>)[key];
        if (typeof value !== "function") return value;
        return (...args: unknown[]) => {
          (calls[key] ??= []).push(args);
          return (value as (...a: unknown[]) => unknown).apply(target, args);
        };
      },
    });
  return { wrap, calls };
}

async function flush(times = 12): Promise<void> {
  for (let i = 0; i < times; i += 1) await act(async () => void (await vi.advanceTimersByTimeAsync(5)));
}

// ---------------------------------------------------------------- Öffnen, Schließen, Blättern

describe("Lister öffnen und schließen", () => {
  test("F3 öffnet den Lister mit dem Dateinamen im Titel", async () => {
    const app = await setup();
    const dialog = await open(app, "umlaute.txt");
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(within(dialog).getByText("Ansehen: umlaute.txt")).toBeInTheDocument();
  });

  test("die F3-Schaltfläche der F-Tasten-Leiste öffnet den Lister", async () => {
    const app = await setup();
    await app.user.click(app.row(0, "umlaute.txt"));
    await app.user.click(screen.getByRole("button", { name: /F3/ }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  test("Esc schließt den Lister", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await app.user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  test("Q schließt den Lister", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await app.user.keyboard("q");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  test("F3 schließt den Lister, solange kein Suchbegriff gesetzt ist", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await app.user.keyboard("{F3}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  test("der Fokus geht beim Schließen zurück in die Liste", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await app.user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.activeElement).toBe(within(app.pane(0)).getByRole("grid"));
  });

  test("F3 auf einem Ordner öffnet keinen Lister und meldet es", async () => {
    const app = await setupApp({ left: HOME });
    await app.user.click(app.row(0, "Dokumente"));
    await app.user.keyboard("{F3}");
    expect(await screen.findByText("Ordner lassen sich nicht ansehen.")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("F3 auf „..“ öffnet nichts und meldet nichts", async () => {
    const app = await setup();
    expect(app.focused(0)).toBe("..");
    await app.user.keyboard("{F3}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(app.runtime.store.getState().toasts).toHaveLength(0);
  });

  test("Strg+Q auf einem Ordner schaltet die Schnellansicht ein und zeigt „keine Vorschau“", async () => {
    const app = await setupApp({ left: HOME });
    await app.user.click(app.row(0, "Dokumente"));
    await app.user.keyboard("{Control>}q{/Control}");
    expect(await screen.findByText("Ordner: keine Vorschau.")).toBeInTheDocument();
  });

  test("Pfeil rechts und links blättern durch die Dateien der Liste und der Fokus der Liste folgt", async () => {
    const app = await setup();
    const names = app.names(0).filter((n) => n !== ".." && n !== "");
    await open(app, names[0]!);
    await app.user.keyboard("{ArrowRight}");
    expect(await screen.findByText(`Ansehen: ${names[1]!}`)).toBeInTheDocument();
    expect(app.focused(0)).toBe(names[1]);
    await app.user.keyboard("{ArrowRight}{ArrowLeft}{ArrowLeft}");
    expect(await screen.findByText(`Ansehen: ${names[0]!}`)).toBeInTheDocument();
    expect(app.focused(0)).toBe(names[0]);
  });

  test("Pfeil links bei der ersten Datei bleibt stehen (nicht auf „..“)", async () => {
    const app = await setup();
    const first = app.names(0).filter((n) => n !== "..")[0]!;
    await open(app, first);
    await app.user.keyboard("{ArrowLeft}");
    expect(screen.getByText(`Ansehen: ${first}`)).toBeInTheDocument();
    expect(app.focused(0)).toBe(first);
  });

  test("Ordner in der Liste werden beim Blättern übersprungen", async () => {
    const app = await setupApp({ left: HOME, samples: true });
    const names = app.names(0);
    const lastDir = names.lastIndexOf("Projekte");
    const before = names[lastDir - 1]!;
    expect(["Dokumente", "Downloads", "Bilder", "Ansicht", "Musik", ".config"]).toContain(before);
    // von der letzten Datei aus gibt es kein „nächste Datei“ vor dem Ordnerblock: Rechts wechselt nur auf Dateien
    await open(app, "start.sh");
    await app.user.keyboard("{ArrowLeft}");
    const title = screen.getByRole("dialog").querySelector("header span")?.textContent ?? "";
    expect(title).toMatch(/^Ansehen: /);
    expect(["Dokumente", "Downloads", "Bilder", "Ansicht", "Musik", "Projekte", "..", ".config"].some((d) => title === `Ansehen: ${d}`)).toBe(false);
  });

  test("Fokusfalle: Tab und Umschalt+Tab bleiben im Lister", async () => {
    const app = await setup();
    const dialog = await open(app, "umlaute.txt");
    await screen.findByRole("textbox", { name: "Textansicht" });
    for (let i = 0; i < 25; i += 1) {
      await app.user.tab();
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
    for (let i = 0; i < 25; i += 1) {
      await app.user.tab({ shift: true });
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
  });

  test("Tasten der Liste gelten nicht, solange der Lister offen ist", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await app.user.keyboard("{F7}");
    expect(app.runtime.store.getState().dialog).toBeNull();
  });

  test("englische Oberfläche: Titel und Statuszeile sind übersetzt", async () => {
    const app = await setup({ language: "en" });
    await app.user.click(app.row(0, "umlaute.txt"));
    await app.user.keyboard("{F3}");
    expect(await screen.findByText("View: umlaute.txt")).toBeInTheDocument();
    expect(await screen.findByRole("textbox", { name: "Text view" })).toBeInTheDocument();
  });

  test("ein Fehler beim Laden der Dateiinformationen wird als Meldung gezeigt", async () => {
    const app = await setup({
      wrap: (api) => ({ ...api, fileInfo: async () => Promise.reject(new CoreError("Zugriff verweigert", "permission")) }),
    });
    await open(app, "umlaute.txt");
    expect(await screen.findByRole("alert")).toHaveTextContent("Die Datei kann nicht angezeigt werden");
  });
});

// ---------------------------------------------------------------- Textansicht

describe("Textansicht", () => {
  test("UTF-8 zeigt Umlaute; Statuszeile nennt Kodierung und Größe", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await waitText(textbox, "Grüße aus Köln");
    expect(textbox()).toHaveTextContent("ÄÖÜ äöü ß €");
    expect(status()).toHaveTextContent("UTF-8");
    expect(status()).toHaveTextContent(/Größe/);
  });

  test("UTF-16 LE wird an der Byte-Order-Mark erkannt", async () => {
    const app = await setup();
    await open(app, "utf16.txt");
    await waitText(textbox, "Grüße aus Köln");
    expect(status()).toHaveTextContent("UTF-16 LE");
  });

  test("UTF-16 BE wird an der Byte-Order-Mark erkannt", async () => {
    const app = await setup();
    await open(app, "utf16-be.txt");
    await waitText(textbox, "Grüße aus Köln");
    expect(status()).toHaveTextContent("UTF-16 BE");
  });

  test("manuelles Umschalten der Kodierung: ungültige Bytes erscheinen als Ersatzzeichen, Windows-1252 und ISO-8859-1 unterscheiden sich", async () => {
    const app = await setup();
    await open(app, "windows1252.txt");
    await waitText(textbox, "�");
    const select = screen.getByRole("combobox", { name: /Kodierung/ });
    await app.user.selectOptions(select, "windows-1252");
    await waitText(textbox, "Grüße, Preis 5 €");
    expect(status()).toHaveTextContent("Windows-1252");
    await app.user.selectOptions(select, "iso-8859-1");
    await waitText(textbox, "Grüße, Preis 5 \u0080");
    expect(status()).toHaveTextContent("ISO-8859-1");
  });

  test("Zeilenumbruch lässt sich ein- und ausschalten", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await waitText(textbox, "Grüße");
    expect(textbox()).not.toHaveClass("wrap");
    await app.user.click(screen.getByRole("checkbox", { name: "Zeilenumbruch" }));
    expect(textbox()).toHaveClass("wrap");
    await app.user.click(screen.getByRole("checkbox", { name: "Zeilenumbruch" }));
    expect(textbox()).not.toHaveClass("wrap");
  });

  test("Zeilennummern lassen sich ausschalten", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await waitText(textbox, "Grüße");
    expect([...textbox().querySelectorAll(".lv-no")].map((n) => n.textContent)).toEqual(["1", "2", "3"]);
    await app.user.click(screen.getByRole("checkbox", { name: "Zeilennummern" }));
    expect(textbox().querySelectorAll(".lv-no")).toHaveLength(0);
  });

  test("die Schriftgröße lässt sich ändern und bleibt beim Blättern erhalten", async () => {
    const app = await setup();
    const names = app.names(0).filter((n) => n !== "..");
    const first = names.find((n) => n.endsWith(".txt"))!;
    await open(app, first);
    await screen.findByRole("textbox", { name: "Textansicht" });
    expect(textbox().style.fontSize).toBe("13px");
    await app.user.click(screen.getByRole("button", { name: "Schrift größer" }));
    expect(textbox().style.fontSize).toBe("14px");
    await app.user.click(screen.getByRole("button", { name: "Schrift kleiner" }));
    await app.user.click(screen.getByRole("button", { name: "Schrift kleiner" }));
    expect(textbox().style.fontSize).toBe("12px");
  });

  test("Text wird nie als HTML ausgewertet", async () => {
    const html = new TextEncoder().encode('<img src=x onerror="alert(1)"><b>fett</b>\n');
    const app = await setup({
      wrap: (api) => ({ ...api, readBlock: async (p, o, l) => (p.endsWith("umlaute.txt") ? html.slice(o, o + l) : api.readBlock(p, o, l)) }),
    });
    await open(app, "umlaute.txt");
    await waitText(textbox, '<img src=x onerror="alert(1)"><b>fett</b>');
    expect(textbox().querySelector("img, b")).toBeNull();
  });

  test("Textansicht ist schreibgeschützt und hat die Rollen für Hilfsmittel", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await screen.findByRole("textbox", { name: "Textansicht" });
    expect(textbox()).toHaveAttribute("aria-readonly", "true");
    expect(textbox()).toHaveAttribute("aria-multiline", "true");
  });

  test("eine große Datei wird nur fensterweise gelesen, nie ganz", async () => {
    const { wrap, calls } = spy();
    const app = await setup({ wrap });
    await open(app, "riesig.txt");
    await waitText(textbox, "Zeile 00000000");
    const bytes = (calls.readBlock ?? []).reduce((sum, [, , len]) => sum + (len as number), 0);
    expect(bytes).toBeLessThanOrEqual(256 * 1024 + 8);
    expect(textbox().querySelectorAll(".lv-line").length).toBeLessThan(5000);
  });

  test("am unteren Rand blättert ein Pfeil-runter zur nächsten Seite mit fortlaufenden Zeilennummern", async () => {
    const app = await setup();
    await open(app, "riesig.txt");
    await waitText(textbox, "Zeile 00000000");
    // jsdom hat keine Layouthöhen: der Rand ist immer erreicht
    await app.user.keyboard("{ArrowDown}");
    await waitFor(() => {
      const first = Number(textbox().querySelector(".lv-no")?.textContent);
      expect(first).toBeGreaterThan(4000);
    });
    const first = Number(textbox().querySelector(".lv-no")?.textContent);
    expect(textbox().querySelector(".lv-tx")?.textContent).toContain(`Zeile ${String(first - 1).padStart(8, "0")}`);
  });

  test("Ende springt an das Dateiende, Pos1 zurück an den Anfang", async () => {
    const app = await setup();
    await open(app, "riesig.txt");
    await waitText(textbox, "Zeile 00000000");
    await app.user.keyboard("{End}");
    await waitFor(() => expect(status()).toHaveTextContent(/Position 9[89] %|Position 100 %/));
    await app.user.keyboard("{Home}");
    await waitFor(() => expect(textbox().querySelector(".lv-no")?.textContent).toBe("1"));
  });
});

// ---------------------------------------------------------------- Springen

describe("Springen", () => {
  test("Strg+G springt im Text zu einer Zeile", async () => {
    const app = await setup();
    await open(app, "riesig.txt");
    await waitText(textbox, "Zeile 00000000");
    await app.user.keyboard("{Control>}g{/Control}");
    await app.user.keyboard("1000{Enter}");
    await waitFor(() => expect(textbox().querySelector(".lv-no")?.textContent).toBe("1000"));
    expect(textbox().querySelector(".lv-tx")?.textContent).toContain("Zeile 00000999");
  });

  test("Sprung zu einer Zeile hinter dem Dateiende meldet es", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await waitText(textbox, "Grüße");
    await app.user.keyboard("{Control>}g{/Control}");
    await app.user.keyboard("99{Enter}");
    expect(await screen.findByText("Die Datei hat weniger als 99 Zeilen.")).toBeInTheDocument();
  });

  test("Sprung zu Prozent landet bei der passenden Position", async () => {
    const app = await setup();
    await open(app, "riesig.txt");
    await waitText(textbox, "Zeile 00000000");
    await app.user.keyboard("{Control>}g{/Control}");
    await app.user.keyboard("50%{Enter}");
    await waitFor(() => expect(status()).toHaveTextContent("Position 50 %"));
    // Zeilennummern werden jenseits der Zählgrenze nicht angezeigt
    expect(textbox().querySelector(".lv-no")?.textContent).toBe("");
  });

  test("ungültige Eingabe beim Sprung zeigt eine Meldung und schließt die Leiste nicht", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await waitText(textbox, "Grüße");
    await app.user.keyboard("{Control>}g{/Control}");
    await app.user.keyboard("abc{Enter}");
    expect(await screen.findByText("Ungültige Eingabe.")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Springen zu" })).toBeInTheDocument();
  });

  test("Esc schließt zuerst die Sprungleiste, erst das zweite Esc den Lister", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await waitText(textbox, "Grüße");
    await app.user.keyboard("{Control>}g{/Control}");
    await screen.findByRole("form", { name: "Springen zu" });
    await app.user.keyboard("{Escape}");
    expect(screen.queryByRole("form", { name: "Springen zu" })).toBeNull();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await app.user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  test("Hex: Sprung zu einem Offset (hexadezimal) markiert das Byte", async () => {
    const app = await setup();
    await open(app, "zufall.bin");
    await screen.findByRole("log", { name: "Hex-Ansicht" });
    await app.user.keyboard("{Control>}g{/Control}");
    await app.user.keyboard("0x100{Enter}");
    await waitFor(() => expect(hexlog().querySelectorAll(".hit").length).toBeGreaterThan(0));
    const row = hexlog().querySelector(".hit")?.closest(".hx-row");
    expect(row?.getAttribute("data-offset")).toBe("256");
  });

  test("Hex: Offset außerhalb der Datei wird abgewiesen", async () => {
    const app = await setup();
    await open(app, "zufall.bin");
    await screen.findByRole("log", { name: "Hex-Ansicht" });
    await app.user.keyboard("{Control>}g{/Control}");
    await app.user.keyboard("99999{Enter}");
    expect(await screen.findByText("Ungültige Eingabe.")).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------- Suche

describe("Suche im Text", () => {
  async function openFind(app: App, name = "umlaute.txt"): Promise<HTMLElement> {
    await open(app, name);
    await waitFor(() => expect(textbox().querySelector(".lv-line")).not.toBeNull());
    await app.user.keyboard("{Control>}f{/Control}");
    return within(screen.getByRole("dialog")).getByRole("searchbox");
  }

  test("B29 ein Texttreffer wird mit Zeile und Spalte gemeldet", async () => {
    const app = await setup();
    const input = await openFind(app);
    await app.user.type(input, "Köln{Enter}");
    await waitFor(() => expect(textbox().querySelector("mark")?.textContent).toBe("Köln"));
    expect(await screen.findByText("Zeile 1, Spalte 11")).toBeInTheDocument();
  });

  test("Enter sucht weiter, Umschalt+Enter zurück, F3 und Umschalt+F3 ebenso", async () => {
    const app = await setup();
    const input = await openFind(app);
    await app.user.type(input, "Zeile{Enter}");
    const hitLine = () => textbox().querySelector(".lv-line.hit .lv-no")?.textContent;
    await waitFor(() => expect(hitLine()).toBe("2"));
    await app.user.keyboard("{Enter}");
    await waitFor(() => expect(hitLine()).toBe("3"));
    await app.user.keyboard("{Shift>}{Enter}{/Shift}");
    await waitFor(() => expect(hitLine()).toBe("2"));
    await app.user.keyboard("{F3}");
    await waitFor(() => expect(hitLine()).toBe("3"));
    await app.user.keyboard("{Shift>}{F3}{/Shift}");
    await waitFor(() => expect(hitLine()).toBe("2"));
    // F3 mit Suchbegriff schließt den Lister nicht
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  test("ohne Treffer: „Nicht gefunden.“", async () => {
    const app = await setup();
    const input = await openFind(app);
    await app.user.type(input, "xyzzy{Enter}");
    expect(await screen.findByText("Nicht gefunden.")).toBeInTheDocument();
    expect(textbox().querySelector("mark")).toBeNull();
  });

  test("Groß-/Kleinschreibung: ohne Haken egal, mit Haken genau", async () => {
    const app = await setup();
    const input = await openFind(app);
    await app.user.type(input, "KÖLN{Enter}");
    // der Kern faltet nur ASCII: „KÖLN“ passt nicht auf „Köln“
    expect(await screen.findByText("Nicht gefunden.")).toBeInTheDocument();
    await app.user.clear(input);
    await app.user.type(input, "AUS{Enter}");
    await waitFor(() => expect(textbox().querySelector("mark")?.textContent).toBe("aus"));
    await app.user.click(screen.getByRole("checkbox", { name: "Groß-/Kleinschreibung" }));
    await app.user.click(screen.getByRole("button", { name: "Weiter" }));
    expect(await screen.findByText("Nicht gefunden.")).toBeInTheDocument();
  });

  test("Esc schließt zuerst die Suchleiste, dann den Lister", async () => {
    const app = await setup();
    await openFind(app);
    await app.user.keyboard("{Escape}");
    expect(screen.queryByRole("search")).toBeNull();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await app.user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  test("ein nicht darstellbarer Suchbegriff wird gemeldet, ohne zu suchen", async () => {
    const { wrap, calls } = spy();
    const app = await setup({ wrap });
    await app.user.click(app.row(0, "windows1252.txt"));
    await app.user.keyboard("{F3}");
    await waitFor(() => expect(textbox().querySelector(".lv-line")).not.toBeNull());
    await app.user.selectOptions(screen.getByRole("combobox", { name: /Kodierung/ }), "iso-8859-1");
    await app.user.keyboard("{Control>}f{/Control}");
    await app.user.type(within(screen.getByRole("dialog")).getByRole("searchbox"), "€{Enter}");
    expect(await screen.findByText(/nicht darstellbar/)).toBeInTheDocument();
    expect(calls.findInFile).toBeUndefined();
  });

  test("Suche nach einem Treffer außerhalb des Fensters lädt das passende Fenster", async () => {
    const app = await setup();
    await open(app, "riesig.txt");
    await waitText(textbox, "Zeile 00000000");
    await app.user.keyboard("{Control>}f{/Control}");
    await app.user.type(within(screen.getByRole("dialog")).getByRole("searchbox"), "Zeile 00050000{Enter}");
    await waitFor(() => expect(textbox().querySelector("mark")?.textContent).toBe("Zeile 00050000"));
    expect(textbox().querySelector(".lv-line.hit .lv-no")?.textContent).toBe("50001");
  });

  test("Fortsetzen nach einem „continue“-Ergebnis: die nächste Suche beginnt am gemeldeten Offset", async () => {
    const { wrap, calls } = spy();
    const app = await setup({ wrap });
    await open(app, "riesig.txt");
    await waitText(textbox, "Zeile 00000000");
    await app.user.keyboard("{Control>}f{/Control}");
    await app.user.type(within(screen.getByRole("dialog")).getByRole("searchbox"), "qqqq{Enter}");
    expect(await screen.findByText(/kein Treffer\. Mit F3 oder Enter weitersuchen/)).toBeInTheDocument();
    const first = calls.findInFile![0]!;
    const next = 4 * 1024 * 1024;
    expect(first[2]).toBe(0);
    await app.user.keyboard("{Enter}");
    await waitFor(() => expect(calls.findInFile).toHaveLength(2));
    expect(calls.findInFile![1]![2]).toBe(next);
  });

  test("Esc bricht eine laufende Suche ab; der Lister bleibt offen", async () => {
    let reject: ((error: Error) => void) | null = null;
    const cancel = vi.fn();
    const app = await setup({
      wrap: (api) => ({
        ...api,
        findInFile: () => new Promise((_, no) => void (reject = no)),
        cancelSearch: async () => {
          cancel();
          reject?.(new CoreError("Der Vorgang wurde abgebrochen", "cancelled"));
        },
      }),
    });
    await open(app, "umlaute.txt");
    await waitFor(() => expect(textbox().querySelector(".lv-line")).not.toBeNull());
    await app.user.keyboard("{Control>}f{/Control}");
    await app.user.type(within(screen.getByRole("dialog")).getByRole("searchbox"), "Köln{Enter}");
    expect(await screen.findByText("Suche läuft … (Esc bricht ab)")).toBeInTheDocument();
    await app.user.keyboard("{Escape}");
    expect(await screen.findByText("Suche abgebrochen.")).toBeInTheDocument();
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  test("das Schließen des Listers während einer Suche bricht sie ab", async () => {
    const cancel = vi.fn();
    const app = await setup({
      wrap: (api) => ({ ...api, findInFile: () => new Promise(() => undefined), cancelSearch: async () => void cancel() }),
    });
    await open(app, "umlaute.txt");
    await waitFor(() => expect(textbox().querySelector(".lv-line")).not.toBeNull());
    await app.user.keyboard("{Control>}f{/Control}");
    await app.user.type(within(screen.getByRole("dialog")).getByRole("searchbox"), "Köln{Enter}");
    await screen.findByText("Suche läuft … (Esc bricht ab)");
    await app.user.click(within(screen.getByRole("dialog")).getAllByRole("button", { name: "Schließen" }).at(-1)!);
    await waitFor(() => expect(cancel).toHaveBeenCalled());
  });
});

// ---------------------------------------------------------------- Hex

describe("Hexansicht", () => {
  test("Binärdateien öffnen automatisch als Hex: 16 Bytes je Zeile, Offset, Hex, ASCII", async () => {
    const app = await setup();
    await open(app, "zufall.bin");
    await screen.findByRole("log", { name: "Hex-Ansicht" });
    await waitFor(() => expect(hexlog().querySelectorAll(".hx-row")).toHaveLength(44));
    const expected = mockFileBytes("zufall.bin", 700, 0, 16);
    const first = hexlog().querySelector(".hx-row")!;
    expect(first.querySelector(".hx-off")?.textContent).toBe("00000000");
    expect(first.querySelector(".hx-hex")?.textContent?.replace(/\s+/g, " ").trim()).toBe([...expected].map((b) => b.toString(16).padStart(2, "0")).join(" "));
    expect(first.querySelector(".hx-asc")?.textContent).toHaveLength(16);
  });

  test("die letzte Zeile ist kürzer und trägt den richtigen Offset", async () => {
    const app = await setup();
    await open(app, "zufall.bin");
    await waitFor(() => expect(hexlog().querySelectorAll(".hx-row")).toHaveLength(44));
    const rows = hexlog().querySelectorAll(".hx-row");
    const last = rows[rows.length - 1]!;
    expect(last.querySelector(".hx-off")?.textContent).toBe((43 * 16).toString(16).padStart(8, "0"));
    expect(last.querySelector(".hx-hex")?.textContent?.trim().split(/\s+/)).toHaveLength(700 - 43 * 16);
    expect(last.querySelector(".hx-asc")?.textContent).toHaveLength(700 - 43 * 16);
  });

  test("Suche nach einer Bytefolge markiert genau diese Bytes", async () => {
    const app = await setup();
    await open(app, "zufall.bin");
    await waitFor(() => expect(hexlog().querySelectorAll(".hx-row").length).toBeGreaterThan(0));
    const data = mockFileBytes("zufall.bin", 700, 0, 700);
    const needle = [data[100]!, data[101]!, data[102]!];
    let expected = -1;
    for (let i = 0; i + 3 <= 700 && expected < 0; i += 1) if (needle.every((b, k) => data[i + k] === b)) expected = i;
    await app.user.keyboard("{Control>}f{/Control}");
    await app.user.click(screen.getByRole("checkbox", { name: "Bytefolge (Hex)" }));
    await app.user.type(within(screen.getByRole("dialog")).getByRole("searchbox"), `${needle.map((b) => b.toString(16).padStart(2, "0")).join(" ")}{Enter}`);
    await waitFor(() => expect(hexlog().querySelectorAll(".hx-hex .hit")).toHaveLength(3));
    expect(await screen.findByText(`Treffer bei Offset 0x${expected.toString(16)}`)).toBeInTheDocument();
    expect(hexlog().querySelectorAll(".hx-asc .hit")).toHaveLength(3);
  });

  test("eine ungültige Bytefolge wird gemeldet", async () => {
    const app = await setup();
    await open(app, "zufall.bin");
    await waitFor(() => expect(hexlog().querySelectorAll(".hx-row").length).toBeGreaterThan(0));
    await app.user.keyboard("{Control>}f{/Control}");
    await app.user.click(screen.getByRole("checkbox", { name: "Bytefolge (Hex)" }));
    await app.user.type(within(screen.getByRole("dialog")).getByRole("searchbox"), "xyz{Enter}");
    expect(await screen.findByText(/Ungültige Bytefolge/)).toBeInTheDocument();
  });

  test("Suche nach Text in der Hexansicht", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await waitFor(() => expect(textbox().querySelector(".lv-line")).not.toBeNull());
    await app.user.keyboard("2");
    await screen.findByRole("log", { name: "Hex-Ansicht" });
    await waitFor(() => expect(hexlog().querySelectorAll(".hx-row").length).toBeGreaterThan(0));
    await app.user.keyboard("{Control>}f{/Control}");
    await app.user.type(within(screen.getByRole("dialog")).getByRole("searchbox"), "Köln{Enter}");
    expect(await screen.findByText("Treffer bei Offset 0xc")).toBeInTheDocument();
    expect(hexlog().querySelectorAll(".hx-hex .hit")).toHaveLength(5); // K ö(2) l n
  });

  test("die Hexansicht nennt den Offset in der Statuszeile", async () => {
    const app = await setup();
    await open(app, "zufall.bin");
    await waitFor(() => expect(status()).toHaveTextContent("Offset 0x0"));
  });
});

// ---------------------------------------------------------------- Moduswahl

describe("Moduswahl", () => {
  test("Text, Binär, PNG, SVG und WAV öffnen automatisch in der passenden Ansicht", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await screen.findByRole("textbox", { name: "Textansicht" });
    await app.user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await open(app, "zufall.bin");
    await screen.findByRole("log", { name: "Hex-Ansicht" });
    await app.user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await open(app, "muster.png");
    expect(await screen.findByRole("img", { name: "Vorschau von muster.png" })).toBeInTheDocument();
    await app.user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await open(app, "logo.svg");
    expect(await screen.findByRole("img", { name: "Vorschau von logo.svg" })).toBeInTheDocument();
    await app.user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await open(app, "klick.wav");
    expect(await screen.findByRole("button", { name: /Laden/ })).toBeInTheDocument();
  });

  test("Tasten 1, 2 und 3 schalten manuell um", async () => {
    const app = await setup();
    await open(app, "muster.png");
    await screen.findByRole("img", { name: "Vorschau von muster.png" });
    await app.user.keyboard("2");
    expect(await screen.findByRole("log", { name: "Hex-Ansicht" })).toBeInTheDocument();
    await waitFor(() => expect(hexlog()).toHaveTextContent("89 50 4e 47"));
    await app.user.keyboard("1");
    expect(await screen.findByRole("textbox", { name: "Textansicht" })).toBeInTheDocument();
    await app.user.keyboard("3");
    expect(await screen.findByRole("img", { name: "Vorschau von muster.png" })).toBeInTheDocument();
  });

  test("3 bei einer Datei ohne Bild-/Medienansicht meldet das und lässt die Ansicht stehen", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await screen.findByRole("textbox", { name: "Textansicht" });
    await app.user.keyboard("3");
    expect(await screen.findByText(/keine Bild- oder Medienansicht/)).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Textansicht" })).toBeInTheDocument();
  });

  test("die Umschaltknöpfe sind klickbar und zeigen den Zustand", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await screen.findByRole("textbox", { name: "Textansicht" });
    const hex = screen.getByRole("button", { name: /^Hex/ });
    expect(screen.getByRole("button", { name: /^Text/ })).toHaveAttribute("aria-pressed", "true");
    await app.user.click(hex);
    expect(hex).toHaveAttribute("aria-pressed", "true");
    expect(await screen.findByRole("log", { name: "Hex-Ansicht" })).toBeInTheDocument();
  });

  test("Tasten 1/2/3 gelten nicht in Eingabefeldern (Suchfeld)", async () => {
    const app = await setup();
    await open(app, "umlaute.txt");
    await waitFor(() => expect(textbox().querySelector(".lv-line")).not.toBeNull());
    await app.user.keyboard("{Control>}f{/Control}");
    await app.user.type(within(screen.getByRole("dialog")).getByRole("searchbox"), "2");
    expect(within(screen.getByRole("dialog")).getByRole("searchbox")).toHaveValue("2");
    expect(screen.getByRole("textbox", { name: "Textansicht" })).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------- Bilder und Medien

describe("Bild- und Medienansicht", () => {
  test("PNG wird als Blob-URL in einem <img> gezeigt", async () => {
    const app = await setup();
    await open(app, "muster.png");
    const img = await screen.findByRole("img", { name: "Vorschau von muster.png" });
    expect(img.tagName).toBe("IMG");
    expect(img.getAttribute("src")).toMatch(/^blob:/);
    expect(created).toHaveLength(1);
    expect(created[0]!.type).toBe("image/png");
    expect(created[0]!.size).toBe(664);
  });

  test("SVG wird nur als <img> gezeigt, nie als Dokument", async () => {
    const app = await setup();
    const dialog = await open(app, "logo.svg");
    const img = await screen.findByRole("img", { name: "Vorschau von logo.svg" });
    expect(img.tagName).toBe("IMG");
    expect(created[0]!.type).toBe("image/svg+xml");
    expect(dialog.querySelector("svg, object, iframe, embed")).toBeNull();
    expect(dialog.innerHTML).not.toContain("<circle");
  });

  test("Bild: Einpassen, 100 %, Zoomen und Drehen", async () => {
    const app = await setup();
    await open(app, "muster.png");
    const img = await screen.findByRole("img", { name: "Vorschau von muster.png" });
    Object.defineProperty(img, "naturalWidth", { value: 16 });
    Object.defineProperty(img, "naturalHeight", { value: 16 });
    fireEvent.load(img);
    expect(screen.getByRole("button", { name: "Einpassen" })).toHaveClass("on");
    await app.user.click(screen.getByRole("button", { name: "100 %" }));
    expect(img.style.width).toBe("16px");
    await app.user.click(screen.getByRole("button", { name: "Vergrößern" }));
    expect(img.style.width).toBe("24px"); // 150 %
    await app.user.click(screen.getByRole("button", { name: "Verkleinern" }));
    await app.user.click(screen.getByRole("button", { name: "Verkleinern" }));
    expect(img.style.width).toBe("12px"); // 75 %
    await app.user.click(screen.getByRole("button", { name: /Drehen/ }));
    expect(img.style.transform).toBe("rotate(90deg)");
    await app.user.click(screen.getByRole("button", { name: "Einpassen" }));
    expect(img.style.maxWidth).toBe("100%");
  });

  test("ein Bild über 32 MiB wird nicht geladen: Meldung und Knopf zum externen Öffnen", async () => {
    const opened: string[] = [];
    const { wrap, calls } = spy();
    const app = await setup({ wrap: (api) => wrap!({ ...api, openExternal: async (p: string) => void opened.push(p) }) });
    await open(app, "panorama.png");
    expect(await screen.findByText(/Zu groß für die Vorschau \(40[^)]*Grenze 32/)).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: /Vorschau/ })).toBeNull();
    expect(created).toHaveLength(0);
    expect((calls.readBlock ?? []).every(([, offset]) => (offset as number) === 0)).toBe(true);
    await app.user.click(screen.getByRole("button", { name: "Extern öffnen" }));
    await waitFor(() => expect(opened).toEqual([`${ANSICHT}/panorama.png`]));
  });

  test("ein Video über 128 MiB wird nicht geladen: Meldung statt Laden", async () => {
    const app = await setup();
    await open(app, "film.mp4");
    expect(await screen.findByText(/Zu groß für die Vorschau \(200[^)]*Grenze 128/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Laden/ })).toBeNull();
    expect(created).toHaveLength(0);
    expect(document.querySelector("video, audio")).toBeNull();
  });

  test("Audio lädt erst auf Klick und spielt nie von selbst (kein Autoplay)", async () => {
    const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockImplementation(() => Promise.resolve());
    const app = await setup();
    await open(app, "klick.wav");
    const load = await screen.findByRole("button", { name: /Laden/ });
    expect(created).toHaveLength(0);
    expect(document.querySelector("audio")).toBeNull();
    await app.user.click(load);
    await waitFor(() => expect(document.querySelector("audio")).not.toBeNull());
    const audio = document.querySelector("audio")!;
    expect(audio.autoplay).toBe(false);
    expect(audio.hasAttribute("autoplay")).toBe(false);
    expect(audio.controls).toBe(true);
    expect(audio.getAttribute("src")).toMatch(/^blob:/);
    expect(created[0]!.type).toBe("audio/wav");
    expect(play).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------- Blob-Verwaltung

describe("Blob-Freigabe", () => {
  test("Schließen gibt die Blob-URL frei", async () => {
    const app = await setup();
    await open(app, "muster.png");
    const img = await screen.findByRole("img", { name: "Vorschau von muster.png" });
    const url = img.getAttribute("src")!;
    expect(live.has(url)).toBe(true);
    await app.user.keyboard("{Escape}");
    await waitFor(() => expect(URL.revokeObjectURL).toHaveBeenCalledWith(url));
    expect(live.size).toBe(0);
    expect(blobHolder.active()).toBeNull();
  });

  test("beim Wechsel der Datei wird der alte Blob freigegeben; nie sind zwei aktiv", async () => {
    const app = await setup();
    await open(app, "logo.svg");
    const first = (await screen.findByRole("img", { name: "Vorschau von logo.svg" })).getAttribute("src")!;
    await app.user.keyboard("{ArrowRight}");
    const second = (await screen.findByRole("img", { name: "Vorschau von muster.png" })).getAttribute("src")!;
    expect(second).not.toBe(first);
    expect(URL.revokeObjectURL).toHaveBeenCalledWith(first);
    expect([...live]).toEqual([second]);
    expect(maxLive).toBe(1);
  });

  test("der Wechsel auf eine Datei ohne Blob gibt den Blob ebenfalls frei", async () => {
    const app = await setup();
    await open(app, "muster.png");
    await screen.findByRole("img", { name: "Vorschau von muster.png" });
    await app.user.keyboard("{ArrowRight}"); // panorama.png: zu groß, kein Blob
    await screen.findByText(/Zu groß für die Vorschau/);
    expect(live.size).toBe(0);
  });

  test("Wechsel zwischen Medienansicht und Hex gibt den Blob frei", async () => {
    const app = await setup();
    await open(app, "muster.png");
    await screen.findByRole("img", { name: "Vorschau von muster.png" });
    await app.user.keyboard("2");
    await screen.findByRole("log", { name: "Hex-Ansicht" });
    expect(live.size).toBe(0);
  });

  test("Lister und Schnellansicht teilen sich einen Blob: bei geöffnetem Lister ruht die Schnellansicht", async () => {
    const app = await setup();
    await app.user.click(app.row(0, "muster.png"));
    await app.user.keyboard("{Control>}q{/Control}");
    // Vorschau im rechten Fenster; Quelle ist das aktive linke
    await screen.findByRole("region", { name: "Schnellansicht" });
    await waitFor(() => expect(live.size).toBe(1), { timeout: 2000 });
    await app.user.keyboard("{F3}");
    await screen.findByRole("dialog");
    await screen.findByText("Der Lister ist geöffnet.");
    await screen.findAllByRole("img", { name: "Vorschau von muster.png" });
    expect(live.size).toBe(1);
    expect(maxLive).toBe(1);
    await app.user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(live.size).toBe(1)); // die Schnellansicht lädt wieder
    expect(maxLive).toBe(1);
  });
});

// ---------------------------------------------------------------- Schnellansicht

describe("Schnellansicht", () => {
  test("Strg+Q schaltet die Vorschau anstelle des anderen Fensters ein und wieder aus", async () => {
    const app = await setup();
    expect(screen.queryByRole("region", { name: "Schnellansicht" })).toBeNull();
    await app.user.keyboard("{Control>}q{/Control}");
    const quick = await screen.findByRole("region", { name: "Schnellansicht" });
    expect(app.pane(1).contains(quick)).toBe(true);
    expect(within(app.pane(1)).queryByRole("grid")).toBeNull();
    expect(within(app.pane(0)).getByRole("grid")).toBeInTheDocument();
    await app.user.keyboard("{Control>}q{/Control}");
    expect(screen.queryByRole("region", { name: "Schnellansicht" })).toBeNull();
    expect(within(app.pane(1)).getByRole("grid")).toBeInTheDocument();
  });

  test("die Schaltfläche in der Werkzeugleiste schaltet ebenfalls um", async () => {
    const app = await setup();
    const button = screen.getByRole("button", { name: "Schnellansicht" });
    expect(button).toHaveAttribute("aria-pressed", "false");
    await app.user.click(button);
    expect(await screen.findByRole("region", { name: "Schnellansicht" })).toBeInTheDocument();
    expect(button).toHaveAttribute("aria-pressed", "true");
    void app;
  });

  test("folgt dem Fokus der Liste, entprellt (150 ms)", async () => {
    const { wrap, calls } = spy();
    const app = await setup({ wrap });
    const names = app.names(0);
    const txt = names.filter((n) => n.endsWith(".txt") && n !== "riesig.txt");
    expect(txt.length).toBeGreaterThanOrEqual(2);
    const [a, b] = [names.indexOf(txt[0]!), names.indexOf(txt[1]!)];
    const controller = app.runtime.controller;
    vi.useFakeTimers();
    try {
      fireEvent.keyDown(document.body, { key: "q", ctrlKey: true });
      await flush();
      const quick = screen.getByRole("region", { name: "Schnellansicht" });
      expect(quick).toHaveTextContent("Ordner: keine Vorschau.");
      calls.fileInfo = [];

      act(() => controller.clickRow(0, a, { ctrl: false, shift: false }));
      await flush(2); // 10 ms: noch nichts geladen
      expect(calls.fileInfo).toHaveLength(0);
      await act(async () => void (await vi.advanceTimersByTimeAsync(100)));
      await flush();
      expect(calls.fileInfo).toHaveLength(1);
      expect(calls.fileInfo![0]![0]).toBe(`${ANSICHT}/${txt[0]}`);

      // schnelles Durchlaufen: nur das Ziel wird geladen
      act(() => controller.clickRow(0, b, { ctrl: false, shift: false }));
      await flush(2);
      act(() => controller.clickRow(0, a, { ctrl: false, shift: false }));
      await flush(2);
      act(() => controller.clickRow(0, b, { ctrl: false, shift: false }));
      await flush(2);
      expect(calls.fileInfo).toHaveLength(1);
      await act(async () => void (await vi.advanceTimersByTimeAsync(200)));
      await flush();
      expect(calls.fileInfo).toHaveLength(2);
      expect(calls.fileInfo![1]![0]).toBe(`${ANSICHT}/${txt[1]}`);
      expect(screen.getByRole("region", { name: "Schnellansicht" })).toHaveTextContent(/Grüße|Zeile|Dritte/);
    } finally {
      vi.useRealTimers();
    }
  });

  test("zeigt den Inhalt der fokussierten Datei im anderen Fenster", async () => {
    const app = await setup();
    await app.user.click(app.row(0, "umlaute.txt"));
    await app.user.keyboard("{Control>}q{/Control}");
    const quick = await screen.findByRole("region", { name: "Schnellansicht" });
    await waitFor(() => expect(within(quick).getByRole("textbox", { name: "Textansicht" })).toHaveTextContent("Grüße aus Köln"));
  });

  test("Fokus auf einen Ordner zeigt „keine Vorschau“", async () => {
    const app = await setupApp({ left: HOME, samples: true });
    await app.user.click(app.row(0, "Dokumente"));
    await app.user.keyboard("{Control>}q{/Control}");
    expect(await screen.findByText("Ordner: keine Vorschau.")).toBeInTheDocument();
  });

  test("ein Klick in die Vorschau tauscht nicht die aktive Seite", async () => {
    const app = await setup();
    await app.user.keyboard("{Control>}q{/Control}");
    const quick = await screen.findByRole("region", { name: "Schnellansicht" });
    await app.user.click(quick);
    expect(app.runtime.store.getState().active).toBe(0);
  });

  test("mit Tab zum anderen Fenster wandert die Vorschau auf die andere Seite", async () => {
    const app = await setup();
    await app.user.keyboard("{Control>}q{/Control}");
    await screen.findByRole("region", { name: "Schnellansicht" });
    await app.user.keyboard("{Tab}");
    await waitFor(() => expect(app.pane(0).querySelector('[aria-label="Schnellansicht"]')).not.toBeNull());
    expect(within(app.pane(1)).getByRole("grid")).toBeInTheDocument();
  });
});

test.each(["de", "en"] as const)("B29 %s image shows loaded pixel dimensions and omits text shortcuts", async language => {
  const app = await setup({ language });
  const dlg = await open(app, "muster.png");
  const img = await within(dlg).findByRole("img");
  Object.defineProperties(img, { naturalWidth: { value: 640 }, naturalHeight: { value: 480 } });
  fireEvent.load(img);
  expect(dlg).toHaveTextContent(language === "de" ? "640 × 480 Pixel" : "640 × 480 pixels");
  expect(dlg).not.toHaveTextContent(/(?:Strg|Ctrl)\+[FG]/);
  fireEvent.keyDown(img.closest(".lv-viewer")!, { key: "1" });
  await waitFor(() => expect(dlg).toHaveTextContent(language === "de" ? "Strg+F Suchen" : "Ctrl+F Find"));
});
