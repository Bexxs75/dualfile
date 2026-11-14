// Zentraler Verwalter für Blob-URLs des Listers und der Schnellansicht: höchstens ein aktiver
// Blob zugleich (der Speicher liegt in der Oberfläche, siehe docs/PHASE-2.md). Vor einem neuen
// Blob und beim Schließen wird der alte mit `URL.revokeObjectURL` freigegeben.

export interface BlobLease {
  readonly url: string;
  /** Gibt die URL frei, falls sie noch die aktive ist (ein veralteter Besitzer stört den neuen nicht). */
  release(): void;
}

let currentUrl: string | null = null;

function revokeCurrent(): void {
  if (currentUrl === null) return;
  URL.revokeObjectURL(currentUrl);
  currentUrl = null;
}

export const blobHolder = {
  /** Macht den Blob zum aktiven; ein vorher aktiver wird zuerst freigegeben. */
  acquire(blob: Blob): BlobLease {
    revokeCurrent();
    const url = URL.createObjectURL(blob);
    currentUrl = url;
    return {
      url,
      release(): void {
        if (currentUrl === url) revokeCurrent();
      },
    };
  },
  /** Gibt den aktiven Blob frei (falls vorhanden). */
  releaseAll(): void {
    revokeCurrent();
  },
  /** Die aktive URL oder null. */
  active(): string | null {
    return currentUrl;
  },
};
