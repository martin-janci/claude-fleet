import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { parseMarkdown } from './markdown';

describe('Hub text never reaches {@html}', () => {
  // List of components that render conversation or session-provided text.
  // Derived from: grep -l 'ConvItem\|items\|turn\.\|MarkdownView\|MarkdownInline\|ToolLine\|SubagentBlock\|current_activity\|last_prompt\|friendly_name' src/lib/*.svelte
  // These files are scanned to ensure hub data (turns, tool output, session state)
  // is never passed to {@html} blocks. Extend this array when adding new components
  // that render untrusted text from the hub.
  const componentFiles = [
    'ConversationPanel.svelte',
    'ConversationHeader.svelte',
    'MarkdownView.svelte',
    'MarkdownInline.svelte',
    'ToolLine.svelte',
    'SubagentBlock.svelte',
    'SessionRowItem.svelte',
    'SessionDetails.svelte',
    'TerminalView.svelte',
    'AgentPanel.svelte',
    'BackgroundDetail.svelte',
  ];

  it('scanned components contain no {@html} blocks', () => {
    let foundHtml = false;
    const filesWithHtml: string[] = [];

    for (const file of componentFiles) {
      const content = readFileSync(`src/lib/${file}`, 'utf8');
      if (content.includes('{@html')) {
        foundHtml = true;
        filesWithHtml.push(file);
      }
    }

    if (foundHtml) {
      throw new Error(
        `Found {@html} in component(s) that render hub text: ${filesWithHtml.join(', ')}. ` +
          'Hub data must be rendered through MarkdownView.svelte or safe template syntax, never {@html}.',
      );
    }

    expect(foundHtml).toBe(false);
  });

  it('scanned file list is non-empty and all files exist', () => {
    expect(componentFiles.length).toBeGreaterThan(0);

    for (const file of componentFiles) {
      const content = readFileSync(`src/lib/${file}`, 'utf8');
      expect(content.length).toBeGreaterThan(0);
    }
  });

  it('markdown.ts exports the parser used by MarkdownView.svelte', () => {
    expect(typeof parseMarkdown).toBe('function');
  });
});
