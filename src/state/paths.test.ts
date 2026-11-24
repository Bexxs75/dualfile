import { describe, expect, test } from "vitest";
import {
  archiveHostFolder,
  archiveRootOf,
  baseName,
  conflictDisplayPath,
  crumbsOf,
  displayPath,
  formatArchivePath,
  isArchiveName,
  isArchivePath,
  joinPath,
  normalizePath,
  parseArchivePath,
  parentPath,
} from "./paths";

const ZIP = "/home/u/paket.zip";

describe("lokale Pfade (unverändert)", () => {
  test("normalisieren, Eltern, Name, Zusammensetzen", () => {
    expect(normalizePath("/a//b/")).toBe("/a/b");
    expect(parentPath("/a/b")).toBe("/a");
    expect(parentPath("/a")).toBe("/");
    expect(parentPath("/")).toBe("/");
    expect(baseName("/a/b")).toBe("b");
    expect(baseName("/")).toBe("/");
    expect(joinPath("/", "x")).toBe("/x");
    expect(joinPath("/a", "x")).toBe("/a/x");
    expect(isArchivePath("/a/archive:b")).toBe(false);
  });
});

describe("Archivpfade", () => {
  test("Format wie im Kern: Archivdatei kodiert, Innenpfad mit /", () => {
    expect(formatArchivePath("/home/u/a!b%c.zip", "/dir/x.txt")).toBe("archive:/home/u/a%21b%25c.zip!/dir/x.txt");
    expect(archiveRootOf(ZIP)).toBe(`archive:${ZIP}!/`);
  });

  test("Zerlegen und Zusammensetzen ist ein Rundlauf, auch mit ! und %", () => {
    for (const outer of [ZIP, "/x/a!b.zip", "/x/100%.zip", "/x/%21!%25.tar.gz"]) {
      const path = formatArchivePath(outer, "/in/ner");
      expect(parseArchivePath(path)).toEqual({ outer, inner: "/in/ner" });
    }
  });

  test("verschachtelt: die Archivdatei ist selbst ein Archivpfad", () => {
    const inner = formatArchivePath(ZIP, "/in!side.tar");
    const nested = formatArchivePath(inner, "/x/y");
    expect(nested).toBe("archive:archive:/home/u/paket.zip%21/in%21side.tar!/x/y");
    expect(parseArchivePath(nested)).toEqual({ outer: inner, inner: "/x/y" });
    expect(parseArchivePath(inner)).toEqual({ outer: ZIP, inner: "/in!side.tar" });
  });

  test("fehlerhafte Formen werden nicht erkannt", () => {
    expect(parseArchivePath("/lokal")).toBeNull();
    expect(parseArchivePath("archive:/x.zip")).toBeNull(); // kein !
    expect(parseArchivePath("archive:/x%20.zip!/a")).toBeNull(); // fremde Kodierung
    expect(parseArchivePath("archive:/x.zip!a")).toBeNull(); // Innenpfad ohne /
  });

  test("Eltern: Innenordner, dann Archivwurzel, dann der Ordner der Archivdatei", () => {
    const deep = formatArchivePath(ZIP, "/a/b");
    expect(parentPath(deep)).toBe(formatArchivePath(ZIP, "/a"));
    expect(parentPath(formatArchivePath(ZIP, "/a"))).toBe(archiveRootOf(ZIP));
    expect(parentPath(archiveRootOf(ZIP))).toBe("/home/u");
    const weird = "/home/u/a!b%c.zip";
    expect(parentPath(archiveRootOf(weird))).toBe("/home/u");
    expect(parentPath(formatArchivePath(weird, "/x!y"))).toBe(archiveRootOf(weird));
  });

  test("Eltern verschachtelter Archive: eine Ebene zurück", () => {
    const tarInZip = formatArchivePath(ZIP, "/dir/in.tar");
    const root = archiveRootOf(tarInZip);
    expect(parentPath(formatArchivePath(tarInZip, "/q"))).toBe(root);
    // Wurzel des inneren Archivs → Ordner der inneren Archivdatei (im äußeren Archiv)
    expect(parentPath(root)).toBe(formatArchivePath(ZIP, "/dir"));
    expect(parentPath(parentPath(root))).toBe(archiveRootOf(ZIP));
    expect(parentPath(parentPath(parentPath(root)))).toBe("/home/u");
  });

  test("Name und Zusammensetzen", () => {
    expect(baseName(formatArchivePath(ZIP, "/a/b.txt"))).toBe("b.txt");
    expect(baseName(archiveRootOf(ZIP))).toBe("paket.zip");
    expect(joinPath(archiveRootOf(ZIP), "x!y.txt")).toBe(formatArchivePath(ZIP, "/x!y.txt"));
    expect(joinPath(formatArchivePath(ZIP, "/a"), "b")).toBe(formatArchivePath(ZIP, "/a/b"));
    expect(normalizePath(`archive:${ZIP}!/a//b/`)).toBe(formatArchivePath(ZIP, "/a/b"));
  });

  test("Ordner der äußersten Archivdatei", () => {
    expect(archiveHostFolder(formatArchivePath(ZIP, "/a"))).toBe("/home/u");
    expect(archiveHostFolder(formatArchivePath(formatArchivePath(ZIP, "/in.tar"), "/a"))).toBe("/home/u");
    expect(archiveHostFolder("/lokal")).toBeNull();
  });

  test("lesbarer Pfad und Brotkrumen zeigen nie das Rohformat", () => {
    const path = formatArchivePath("/home/u/a!b.zip", "/dir/x");
    expect(displayPath(path)).toBe("/home/u/a!b.zip/dir/x");
    expect(displayPath(archiveRootOf(ZIP))).toBe(ZIP);
    expect(displayPath(formatArchivePath(formatArchivePath(ZIP, "/in.tar"), "/q"))).toBe("/home/u/paket.zip/in.tar/q");
    expect(displayPath("/lokal")).toBe("/lokal");
    const crumbs = crumbsOf(formatArchivePath(ZIP, "/dir/x"));
    expect(crumbs.map((c) => c.label)).toEqual(["/", "home", "u", "paket.zip", "dir", "x"]);
    expect(crumbs.every((c) => !c.label.includes("archive:"))).toBe(true);
    expect(crumbs[2]?.path).toBe("/home/u");
    expect(crumbs[3]?.path).toBe(archiveRootOf(ZIP));
    expect(crumbs[5]?.path).toBe(formatArchivePath(ZIP, "/dir/x"));
    expect(crumbsOf("/a/b").map((c) => c.path)).toEqual(["/", "/a", "/a/b"]);
  });

  test("Archivendungen", () => {
    for (const name of ["a.zip", "A.ZIP", "a.7z", "a.tar", "a.tar.gz", "a.tgz", "a.tar.bz2", "a.tbz2", "a.tar.xz", "a.txz", "a.rar"]) {
      expect(isArchiveName(name), name).toBe(true);
    }
    for (const name of ["a.txt", "zip", ".zip", "a.gz", "a.zip.bak", "tar"]) expect(isArchiveName(name), name).toBe(false);
  });
});


test("conflict paths use a directory boundary and fall back when context is missing", () => {
  expect(conflictDisplayPath("/out/sub/file", "/out/")).toBe("sub/file");
  expect(conflictDisplayPath("/outside/file", "/out")).toBe("/outside/file");
  expect(conflictDisplayPath("/out/file", "/")).toBe("out/file");
  expect(conflictDisplayPath("/out/file")).toBe("/out/file");
});

describe("currentRemotePath", () => {
  const profile = { id: "p1" } as unknown as import("../ipc/types").ConnectionProfile;
  const other = { id: "p2" } as unknown as import("../ipc/types").ConnectionProfile;
  test("maps an old connection id to the live connection of the same profile", async () => {
    const { currentRemotePath } = await import("./paths");
    const conns = { "c1-a": { profile, connected: false, endpoint: "" }, "c2-b": { profile, connected: true, endpoint: "" }, "c3-c": { profile: other, connected: true, endpoint: "" } };
    expect(currentRemotePath("sftp:c1-a!/home/x", conns)).toBe("sftp:c2-b!/home/x");
    expect(currentRemotePath("sftp:c3-c!/y", conns)).toBe("sftp:c3-c!/y");
    expect(currentRemotePath("sftp:zz!/y", conns)).toBe("sftp:zz!/y");
    expect(currentRemotePath("/home/x", conns)).toBe("/home/x");
  });
});

describe("Serverpfade aller Protokolle", () => {
  test.each(["ftp", "ftps", "sftp", "smb", "nfs", "webdav"])("%s: die Wurzel behält ihren Schrägstrich und wird nie als lokaler Pfad gekürzt", (scheme) => {
    const root = `${scheme}:c1-e289b!/`;
    expect(normalizePath(root)).toBe(root);
    expect(parentPath(`${scheme}:c1-e289b!/docs`)).toBe(root);
    expect(joinPath(root, "bild a.jpg")).toBe(`${scheme}:c1-e289b!/bild a.jpg`);
    expect(crumbsOf(`${scheme}:c1-e289b!/docs/x`).map((c) => c.path).at(-1)).toBe(`${scheme}:c1-e289b!/docs/x`);
  });
});
