import { useEffect, useRef, useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { fmt } from "../lib/format";
import { useResource } from "../lib/useResource";
import { Failure } from "../ui/Feedback";
import { Ago } from "../ui/Table";
import "../styles/volumes.css";

type Volume = Schemas["TextVolumeView"];
type Listing = Schemas["TextListing"];
type TextFile = Schemas["TextFileView"];
type TextDocument = Schemas["TextDocumentView"];

const TEXT_JOBS = ["filing_documents"];
const EDGAR = "https://www.sec.gov/Archives/edgar/data";

type Place = { volume: string; prefix: string };

const nameOf = (volume: Volume) => volume.path.split("/").filter(Boolean).at(-1) ?? volume.path;

export function TextVolumes() {
  const volumes = useResource(() => must(api.GET("/api/data/text")), []);
  const [open, setOpen] = useState(false);
  const [place, setPlace] = useState<Place>();
  const [file, setFile] = useState<TextFile>();
  const [reading, setReading] = useState<TextDocument>();
  const [changed, setChanged] = useState(0);

  useSpineEvents((event) => {
    if (event.type === "job" && TEXT_JOBS.includes(event.job.name) && event.job.last_finished) {
      volumes.reload();
      setChanged((count) => count + 1);
    }
  });

  if (volumes.error && !volumes.data) {
    return <Failure message={volumes.error} onRetry={volumes.reload} />;
  }
  const list = volumes.data ?? [];
  const first = list.find((volume) => volume.mounted) ?? list[0];
  if (!first) {
    return null;
  }
  const current = place ?? { volume: first.id, prefix: "" };
  const go = (next: Place) => (setPlace(next), setFile(undefined));
  const files = list.reduce((sum, volume) => sum + volume.files, 0);
  const documents = list.reduce((sum, volume) => sum + volume.documents, 0);

  return (
    <section className="volumes" aria-label="Text volumes">
      <header className="volumes-head">
        <h2>Text volumes</h2>
        <ul className="volume-meters">
          {list.map((volume) => (
            <li key={volume.id}>
              <Meter volume={volume} />
            </li>
          ))}
        </ul>
        <span className="muted num volumes-totals">
          {fmt.integer(files)} files · {fmt.integer(documents)} documents
        </span>
        <button type="button" className="btn btn-sm" aria-expanded={open} aria-controls="volume-browser" onClick={() => setOpen(!open)}>
          {open ? "Hide files" : "Browse files"}
        </button>
      </header>
      {open && (
        <div id="volume-browser" className="volume-browser">
          <div className="segmented volume-tabs" role="tablist" aria-label="Volume">
            {list.map((volume) => (
              <button key={volume.id} type="button" role="tab" aria-selected={current.volume === volume.id} className={current.volume === volume.id ? "active" : undefined} title={volume.path} onClick={() => go({ volume: volume.id, prefix: "" })}>
                {nameOf(volume)}
              </button>
            ))}
          </div>
          <Folder place={current} file={file} changed={changed} volume={list.find((volume) => volume.id === current.volume)} onGo={go} onFile={setFile} onRead={setReading} />
        </div>
      )}
      {reading && <Reader document={reading} onClose={() => setReading(undefined)} />}
    </section>
  );
}

function Meter({ volume }: { volume: Volume }) {
  const used = volume.total && volume.available !== undefined && volume.available !== null ? volume.total - volume.available : undefined;
  const share = used !== undefined && volume.total ? used / volume.total : 0;
  const label = volume.mounted
    ? `${volume.path}: ${fmt.bytes(used)} used of ${fmt.bytes(volume.total)}, ${fmt.bytes(volume.bytes)} of it text files; no new files above ${Math.round(volume.fill_limit * 100)}% full`
    : `${volume.path}: not mounted`;
  return (
    <span className={volume.mounted ? "volume-meter" : "volume-meter missing"} title={label}>
      <span className="volume-name">{nameOf(volume)}</span>
      <span className="volume-bar" role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={1} aria-valuenow={share}>
        <span className="volume-bar-used" style={{ width: `${share * 100}%` }} />
        <span className="volume-bar-limit" style={{ left: `${volume.fill_limit * 100}%` }} />
      </span>
      <span className="volume-free num">{volume.mounted ? `${fmt.bytes(volume.available)} free` : "not mounted"}</span>
    </span>
  );
}

type FolderProps = {
  place: Place;
  file: TextFile | undefined;
  changed: number;
  volume: Volume | undefined;
  onGo: (place: Place) => void;
  onFile: (file: TextFile | undefined) => void;
  onRead: (document: TextDocument) => void;
};

function Folder({ place, file, changed, volume, onGo, onFile, onRead }: FolderProps) {
  const listing = useResource(() => must(api.GET("/api/data/text/{volume}", { params: { path: { volume: place.volume }, query: { prefix: place.prefix } } })), [place.volume, place.prefix, changed]);
  const parts = place.prefix ? place.prefix.split("/") : [];
  return (
    <div className="folder">
      <nav className="crumbs" aria-label="Folder">
        <button type="button" className="crumb" title={volume?.path} onClick={() => onGo({ volume: place.volume, prefix: "" })}>
          {volume ? nameOf(volume) : place.volume}
        </button>
        {parts.map((part, index) => (
          <span key={index} className="crumb-step">
            <span aria-hidden="true">/</span>
            <button type="button" className="crumb" onClick={() => onGo({ volume: place.volume, prefix: parts.slice(0, index + 1).join("/") })}>
              {part}
            </button>
          </span>
        ))}
        {file && (
          <span className="crumb-step">
            <span aria-hidden="true">/</span>
            <span className="crumb current">{file.path.split("/").at(-1)}</span>
          </span>
        )}
      </nav>
      {file ? <Documents file={file} onBack={() => onFile(undefined)} onRead={onRead} /> : <Entries listing={listing.data} error={listing.error} onOpen={(name) => onGo({ volume: place.volume, prefix: [...parts, name].join("/") })} onFile={onFile} />}
    </div>
  );
}

function Entries({ listing, error, onOpen, onFile }: { listing: Listing | undefined; error: string | undefined; onOpen: (name: string) => void; onFile: (file: TextFile) => void }) {
  if (error) {
    return <p className="muted">{error}</p>;
  }
  if (!listing) {
    return <p className="muted">Loading…</p>;
  }
  if (listing.folders.length === 0 && listing.files.length === 0) {
    return <p className="muted">Nothing stored here yet.</p>;
  }
  return (
    <table className="entries">
      <thead>
        <tr>
          <th scope="col">Name</th>
          <th scope="col" className="num">
            Files
          </th>
          <th scope="col" className="num">
            Documents
          </th>
          <th scope="col" className="num">
            Size
          </th>
          <th scope="col">Written</th>
        </tr>
      </thead>
      <tbody>
        {listing.folders.map((folder) => (
          <tr key={`folder-${folder.name}`}>
            <th scope="row">
              <button type="button" className="entry folder-entry" onClick={() => onOpen(folder.name)}>
                {folder.name}/
              </button>
            </th>
            <td className="num">{fmt.integer(folder.files)}</td>
            <td className="num">{fmt.integer(folder.documents)}</td>
            <td className="num">{fmt.bytes(folder.bytes)}</td>
            <td />
          </tr>
        ))}
        {listing.files.map((file) => (
          <tr key={`file-${file.id}`}>
            <th scope="row">
              <button type="button" className="entry" onClick={() => onFile(file)}>
                {file.path.split("/").at(-1)}
              </button>
            </th>
            <td className="num">1</td>
            <td className="num">{fmt.integer(file.documents)}</td>
            <td className="num">{fmt.bytes(file.bytes)}</td>
            <td className="muted">{file.written_at && <Ago at={file.written_at} />}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Documents({ file, onBack, onRead }: { file: TextFile; onBack: () => void; onRead: (document: TextDocument) => void }) {
  const documents = useResource(() => must(api.GET("/api/data/text/files/{id}", { params: { path: { id: file.id } } })), [file.id]);
  return (
    <div className="documents">
      <button type="button" className="btn btn-ghost btn-sm" onClick={onBack}>
        Back to folder
      </button>
      {documents.error && <p className="muted">{documents.error}</p>}
      {documents.data && (
        <table className="entries">
          <thead>
            <tr>
              <th scope="col">Document</th>
              <th scope="col">Type</th>
              <th scope="col">Filing</th>
              <th scope="col">Public at</th>
              <th scope="col" className="num">
                As filed
              </th>
              <th scope="col" className="num">
                Text
              </th>
            </tr>
          </thead>
          <tbody>
            {documents.data.map((document) => (
              <tr key={document.doc_id}>
                <th scope="row">
                  <button type="button" className="entry" onClick={() => onRead(document)} title={document.description ?? document.document}>
                    {document.document}
                  </button>
                </th>
                <td>{document.kind}</td>
                <td className="num muted">{document.accession}</td>
                <td className="num">{fmt.market(document.published_at)}</td>
                <td className="num">{fmt.bytes(document.raw_bytes)}</td>
                <td className="num">{document.text_chars === null || document.text_chars === undefined ? <span className="muted">none</span> : `${fmt.integer(document.text_chars)} chars`}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function Reader({ document, onClose }: { document: TextDocument; onClose: () => void }) {
  const ref = useRef<HTMLDialogElement>(null);
  const text = useResource(() => must(api.GET("/api/data/text/document", { params: { query: { id: document.doc_id } } })), [document.doc_id]);
  useEffect(() => {
    const dialog = ref.current;
    dialog?.showModal();
    return () => dialog?.close();
  }, []);
  const original = document.cik !== null && document.cik !== undefined && document.accession ? `${EDGAR}/${document.cik}/${document.accession.replaceAll("-", "")}/${document.document}` : undefined;
  return (
    <dialog
      ref={ref}
      className="dialog reader"
      aria-label={document.document}
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      onClick={(event) => event.target === event.currentTarget && onClose()}
    >
      <header className="reader-head">
        <div>
          <h2 className="dialog-title">{document.document}</h2>
          <p className="muted">
            {document.kind}
            {document.description && ` · ${document.description}`} · public {fmt.market(document.published_at)}
          </p>
        </div>
        {original && (
          <a className="btn btn-sm" href={original} target="_blank" rel="noreferrer">
            As filed on EDGAR
          </a>
        )}
        <button type="button" className="btn btn-sm" onClick={onClose} autoFocus>
          Close
        </button>
      </header>
      <div className="reader-body">
        {text.error && <p className="muted">{text.error}</p>}
        {text.data && (text.data.text ? <pre className="reader-text">{text.data.text}</pre> : <p className="muted">This document has no text: it is kept as filed only.</p>)}
        {text.data?.truncated && <p className="muted">Showing the first {fmt.integer(text.data.text?.length ?? 0)} of {fmt.integer(text.data.chars)} characters.</p>}
        {!text.data && !text.error && <p className="muted">Loading…</p>}
      </div>
    </dialog>
  );
}
