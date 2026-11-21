import { useController, useLanguage, useT } from "../../app/context";
import { currentVersion, releaseNotes } from "../../release-notes";
import { Modal } from "../Modal";

export function WhatsNewDialog({ versions, onClose }: { versions: readonly string[]; onClose?: () => void }) {
  const controller = useController(), tr = useT(), language = useLanguage();
  const close = onClose ?? (() => controller.closeDialog());
  return <Modal title={tr("whatsNew.heading", { version: currentVersion })} className="whats-new"
    onClose={close}
    footer={<button type="button" className="tb primary" data-autofocus onClick={close}>{tr("whatsNew.close")}</button>}>
    {versions.length > 1 && <p>{tr("whatsNew.skipped")}</p>}
    {versions.map(version => {
      const note = releaseNotes[version];
      if (!note) return null;
      return <section key={version}>
        <h2>{version} <small>· <time dateTime={note.date}>{new Intl.DateTimeFormat(language, { dateStyle: "long", timeZone: "UTC" }).format(new Date(note.date))}</time></small></h2>
        {(["new", "improved", "fixed"] as const).map(group => note[language][group].length > 0 && <div key={group}>
          <h3>{tr(`whatsNew.${group}`)}</h3>
          <ul>{note[language][group].map(text => <li key={text}>{text}</li>)}</ul>
        </div>)}
      </section>;
    })}
    <a href={`https://github.com/Bexxs75/dualfile/releases/tag/v${currentVersion}`} onClick={event => {
      event.preventDefault();
      void controller.api.openUrl(event.currentTarget.href).catch(error => controller.toast(controller.describeError(error), "error"));
    }}>{tr("whatsNew.changelog")}</a>
  </Modal>;
}
