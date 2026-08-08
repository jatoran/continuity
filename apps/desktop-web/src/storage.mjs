import { createHash } from "node:crypto";
import { mkdir, open, readFile, rename, rm } from "node:fs/promises";
import { dirname, join } from "node:path";

export const STORAGE_VERSION = 1;

export class DurableDocumentStore {
  #directory;
  #durableSequence = 0;
  #rendererSequence = 0;
  #revision = 0;

  constructor(directory) {
    this.#directory = directory;
  }

  async load() {
    await mkdir(this.#directory, { recursive: true });
    const candidates = await Promise.all(this.#candidatePaths().map(readCandidate));
    const valid = candidates.filter((candidate) => candidate.document !== undefined);
    valid.sort((left, right) => right.document.durableSequence - left.document.durableSequence);
    const selected = valid[0]?.document ?? defaultDocument();
    this.#durableSequence = selected.durableSequence;
    this.#revision = selected.revision;
    this.#rendererSequence = 0;
    return {
      ...selected,
      recovery: {
        recovered: candidates.some((candidate) => candidate.isCorrupt),
        discardedCandidates: candidates.filter((candidate) => candidate.isCorrupt).length,
      },
    };
  }

  async persist(detail, associatedFile) {
    validateChange(detail, this.#rendererSequence, this.#revision);
    const document = await this.#writeDocument({
      storageVersion: STORAGE_VERSION,
      durableSequence: this.#durableSequence + 1,
      sourceSequence: detail.sequence,
      revision: detail.snapshot.revision,
      text: detail.snapshot.text,
      selections: detail.snapshot.selections,
      isReadOnly: detail.snapshot.isReadOnly,
      checksumAfter: detail.change.checksumAfter,
      associatedFile: associatedFile ?? null,
      savedAt: new Date().toISOString(),
    });
    this.#rendererSequence = detail.sequence;
    this.#revision = document.revision;
    return publicAcknowledgement(document);
  }

  async persistMetadata(snapshot, associatedFile) {
    if (snapshot?.revision !== this.#revision || typeof snapshot.text !== "string"
        || !Array.isArray(snapshot.selections)) {
      throw new Error("metadata snapshot does not match the durable revision");
    }
    const document = await this.#writeDocument({
      storageVersion: STORAGE_VERSION,
      durableSequence: this.#durableSequence + 1,
      sourceSequence: this.#rendererSequence,
      revision: snapshot.revision,
      text: snapshot.text,
      selections: snapshot.selections,
      isReadOnly: Boolean(snapshot.isReadOnly),
      checksumAfter: "metadata-only",
      associatedFile: associatedFile ?? null,
      savedAt: new Date().toISOString(),
    });
    return publicAcknowledgement(document);
  }

  async simulateInterruptedWrite() {
    const nextSlot = (this.#durableSequence + 1) % 2;
    const handle = await open(this.#temporaryPath(nextSlot), "w");
    try {
      await handle.writeFile('{"storageVersion":1,"durableSequence":', "utf8");
      await handle.sync();
    } finally {
      await handle.close();
    }
  }

  #slotPath(slot) {
    return join(this.#directory, `document-${slot}.json`);
  }

  #temporaryPath(slot) {
    return join(this.#directory, `document-${slot}.json.next`);
  }

  #candidatePaths() {
    return [this.#slotPath(0), this.#slotPath(1), this.#temporaryPath(0), this.#temporaryPath(1)];
  }

  async #writeDocument(payload) {
    const document = { ...payload, integrity: computeIntegrity(payload) };
    const slot = document.durableSequence % 2;
    await writeDurableJson(this.#slotPath(slot), this.#temporaryPath(slot), document);
    this.#durableSequence = document.durableSequence;
    return document;
  }
}

export function computeIntegrity(payload) {
  return createHash("sha256").update(JSON.stringify(payload)).digest("hex");
}

async function readCandidate(path) {
  try {
    const parsed = JSON.parse(await readFile(path, "utf8"));
    const { integrity, ...payload } = parsed;
    if (!isDocumentPayload(payload) || integrity !== computeIntegrity(payload)) {
      return { path, isCorrupt: true };
    }
    return { path, isCorrupt: false, document: parsed };
  } catch (error) {
    if (error?.code === "ENOENT") {
      return { path, isCorrupt: false };
    }
    return { path, isCorrupt: true };
  }
}

async function writeDurableJson(destination, temporary, value) {
  await mkdir(dirname(destination), { recursive: true });
  const handle = await open(temporary, "w");
  try {
    await handle.writeFile(`${JSON.stringify(value)}\n`, "utf8");
    await handle.sync();
  } finally {
    await handle.close();
  }
  await rm(destination, { force: true });
  await rename(temporary, destination);
}

function validateChange(detail, rendererSequence, revision) {
  if (detail?.version !== 1 || !Number.isSafeInteger(detail.sequence)
      || detail.sequence <= rendererSequence) {
    throw new Error("persistence change sequence is invalid or out of order");
  }
  const snapshot = detail.snapshot;
  const change = detail.change;
  if (typeof snapshot?.text !== "string" || !Number.isSafeInteger(snapshot.revision)
      || snapshot.revision < 0 || !Array.isArray(snapshot.selections)
      || change?.revisionAfter !== snapshot.revision
      || change?.revisionBefore !== revision || typeof change.checksumAfter !== "string") {
    throw new Error("persistence change does not continue the durable revision");
  }
}

function isDocumentPayload(value) {
  return value?.storageVersion === STORAGE_VERSION
    && Number.isSafeInteger(value.durableSequence) && value.durableSequence >= 0
    && Number.isSafeInteger(value.revision) && value.revision >= 0
    && typeof value.text === "string" && Array.isArray(value.selections)
    && typeof value.isReadOnly === "boolean" && typeof value.savedAt === "string";
}

function defaultDocument() {
  return {
    storageVersion: STORAGE_VERSION,
    durableSequence: 0,
    sourceSequence: 0,
    revision: 0,
    text: "# Continuity\n\n",
    selections: [],
    isReadOnly: false,
    checksumAfter: "initial",
    associatedFile: null,
    savedAt: new Date(0).toISOString(),
    integrity: "",
  };
}

function publicAcknowledgement(document) {
  return {
    durableSequence: document.durableSequence,
    revision: document.revision,
    savedAt: document.savedAt,
  };
}
