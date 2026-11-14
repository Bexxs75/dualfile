import { createContext, useContext, type ReactNode } from "react";
import type { Language, TranslationKey, TranslationParams } from "../i18n";
import { createTranslator, resolveLanguage } from "../i18n";
import type { AppRuntime } from "./runtime";
import { useStoreSelector } from "./store";
import type { AppState } from "./state";

const RuntimeContext = createContext<AppRuntime | null>(null);

export function RuntimeProvider({ runtime, children }: { runtime: AppRuntime; children: ReactNode }) {
  return <RuntimeContext.Provider value={runtime}>{children}</RuntimeContext.Provider>;
}

export function useRuntime(): AppRuntime {
  const runtime = useContext(RuntimeContext);
  if (!runtime) throw new Error("RuntimeProvider fehlt");
  return runtime;
}

export const useController = () => useRuntime().controller;

export function useAppState<R>(selector: (state: AppState) => R): R {
  return useStoreSelector(useRuntime().store, selector);
}

/** Aktive Sprache aus den Einstellungen (Systemsprache bei „auto“). */
export function useLanguage(): Language {
  const setting = useAppState((state) => state.settings.language);
  return resolveLanguage(setting, typeof navigator === "undefined" ? undefined : navigator.language);
}

export function useT(): (key: TranslationKey, params?: TranslationParams) => string {
  const language = useLanguage();
  return createTranslator(language);
}
