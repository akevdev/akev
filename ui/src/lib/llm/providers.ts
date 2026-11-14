/**
 * Display metadata for LLM providers and APIs (the ids come from the daemon's
 * `akev_content::llm`). Marks are plain text monograms tinted with tag tokens — no brand
 * assets.
 */

import type { LlmApi } from '../../api/types';

export type ProviderTint = 'green' | 'orange' | 'blue' | 'purple' | 'grey' | 'red' | 'yellow';

export interface ProviderInfo {
  id: string;
  /** "OpenAI", "Anthropic", "Google Gemini"… */
  name: string;
  /** One or two letters for the mark. */
  mark: string;
  tint: ProviderTint;
  /** Runs on the user's machine / network. */
  local?: boolean;
}

const PROVIDERS: Record<string, Omit<ProviderInfo, 'id'>> = {
  openai: { name: 'OpenAI', mark: 'O', tint: 'green' },
  anthropic: { name: 'Anthropic', mark: 'A', tint: 'orange' },
  google: { name: 'Google Gemini', mark: 'G', tint: 'blue' },
  'vertex-ai': { name: 'Vertex AI', mark: 'V', tint: 'blue' },
  'azure-openai': { name: 'Azure OpenAI', mark: 'Az', tint: 'blue' },
  'aws-bedrock': { name: 'Amazon Bedrock', mark: 'Br', tint: 'orange' },
  openrouter: { name: 'OpenRouter', mark: 'OR', tint: 'purple' },
  groq: { name: 'Groq', mark: 'Gq', tint: 'red' },
  mistral: { name: 'Mistral', mark: 'M', tint: 'orange' },
  deepseek: { name: 'DeepSeek', mark: 'DS', tint: 'blue' },
  xai: { name: 'xAI', mark: 'x', tint: 'grey' },
  together: { name: 'Together AI', mark: 'T', tint: 'purple' },
  fireworks: { name: 'Fireworks AI', mark: 'F', tint: 'red' },
  perplexity: { name: 'Perplexity', mark: 'P', tint: 'green' },
  cerebras: { name: 'Cerebras', mark: 'C', tint: 'orange' },
  huggingface: { name: 'Hugging Face', mark: 'HF', tint: 'yellow' },
  ollama: { name: 'Ollama', mark: 'Ol', tint: 'grey', local: true },
  lmstudio: { name: 'LM Studio', mark: 'LM', tint: 'purple', local: true },
  'openai-compatible': { name: 'OpenAI-compatible', mark: 'AI', tint: 'grey' },
  'anthropic-compatible': { name: 'Anthropic-compatible', mark: 'AI', tint: 'grey' },
};

/** Display info for a provider id (unknown ids get a generic grey mark). */
export function providerInfo(id: string): ProviderInfo {
  const known = PROVIDERS[id];
  if (known) return { id, ...known };
  const words = id.split(/[-_\s.]+/).filter(Boolean);
  const mark = (words.length > 1 ? words.slice(0, 2).map((w) => w[0]) : [id.slice(0, 2)]).join('').toUpperCase() || 'AI';
  const name = words.map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join(' ') || 'LLM';
  return { id, name, mark, tint: 'grey' };
}

const API_LABELS: Record<LlmApi, string> = {
  'openai-chat': 'Chat Completions',
  'openai-completions': 'Completions',
  'openai-responses': 'Responses API',
  'anthropic-messages': 'Messages API',
  gemini: 'generateContent',
  'ollama-chat': 'Ollama /api/chat',
  'ollama-generate': 'Ollama /api/generate',
  'bedrock-converse': 'Bedrock Converse',
};

/** "Chat Completions", "Messages API"… */
export function apiLabel(api: LlmApi | string): string {
  return API_LABELS[api as LlmApi] ?? api;
}

/** The CSS colour token for a tint (`var(--tag-green)`). */
export function tintVar(tint: ProviderTint): string {
  return `var(--tag-${tint})`;
}
