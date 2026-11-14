import {
  Binary,
  Braces,
  Code,
  FileCode,
  FileText,
  FileType2,
  Film,
  Image,
  Layers,
  Music,
  Paintbrush,
  Radio,
  Table,
  type LucideIcon,
} from 'lucide-react';
import type { ContentKind } from '../../../api/types';

const ICONS: Partial<Record<ContentKind, LucideIcon>> = {
  json: Braces,
  'json-lines': Braces,
  'graph-ql': Braces,
  xml: FileCode,
  html: FileCode,
  javascript: Code,
  css: Paintbrush,
  markdown: FileText,
  yaml: FileText,
  text: FileText,
  csv: Table,
  'form-url-encoded': Table,
  multipart: Layers,
  image: Image,
  audio: Music,
  video: Film,
  font: FileType2,
  'event-stream': Radio,
  protobuf: Binary,
  grpc: Binary,
  'grpc-web': Binary,
  'msg-pack': Binary,
  cbor: Binary,
  wasm: Binary,
  pdf: FileText,
  binary: Binary,
};

/** Icon for a content kind in the Type column (`null` for unknown). */
export function kindIcon(kind: ContentKind): LucideIcon | null {
  return ICONS[kind] ?? null;
}
