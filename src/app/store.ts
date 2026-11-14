import { useSyncExternalStore } from "react";

/** Minimaler Zustandsspeicher: synchron lesbar (`getState`), damit asynchrone Abläufe nie veraltete Daten sehen. */
export interface Store<S, A> {
  getState(): S;
  dispatch(action: A): void;
  subscribe(listener: () => void): () => void;
}

export function createStore<S, A>(reducer: (state: S, action: A) => S, initial: S): Store<S, A> {
  let state = initial;
  const listeners = new Set<() => void>();
  return {
    getState: () => state,
    dispatch(action) {
      const next = reducer(state, action);
      if (next === state) return;
      state = next;
      listeners.forEach((listener) => listener());
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

/** Liest einen Ausschnitt; neu gerendert wird nur, wenn sich der Ausschnitt (per `Object.is`) ändert. */
export function useStoreSelector<S, A, R>(store: Store<S, A>, selector: (state: S) => R): R {
  return useSyncExternalStore(store.subscribe, () => selector(store.getState()));
}
