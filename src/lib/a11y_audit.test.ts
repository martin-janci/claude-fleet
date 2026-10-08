import { describe, it, expect } from 'vitest';
import { a11yReport, accessibleName } from './a11y_audit';

function dom(html: string): HTMLElement {
  const d = document.createElement('div');
  d.innerHTML = html;
  document.body.appendChild(d);
  return d;
}

describe('a11y audit (7.2)', () => {
  it('names a control by labelledby, aria-label, label, text, then title', () => {
    const d = dom(
      '<h4 id="h">Prompt</h4><textarea aria-labelledby="h"></textarea>' +
        '<label for="i">Host</label><input id="i">' +
        '<button><span aria-hidden="true">×</span>Close</button><button title="More"></button>',
    );
    const [ta, input, close, more] = Array.from(d.querySelectorAll('textarea, input, button'));
    expect([ta, input, close, more].map(accessibleName)).toEqual(['Prompt', 'Host', 'Close', 'More']);
    d.remove();
  });

  it('reports what colour or a chevron says alone', () => {
    const d = dom(
      '<select></select>' +
        '<button>▾ Done</button>' +
        '<div class="selected"><button>Row</button></div>' +
        '<span><span class="status-dot"></span></span>' +
        '<button aria-haspopup="menu">Menu</button>' +
        '<label for="g">Type</label><div id="g"></div>',
    );
    expect(a11yReport(d).map((l) => l.split(':')[0])).toEqual([
      'name',
      'popup-expanded',
      'disclosure-expanded',
      'selected-state',
      'status-word',
      'label-for',
    ]);
    d.remove();
  });

  it('passes the same markup once it says its state in words', () => {
    const d = dom(
      '<select aria-label="Host"></select>' +
        '<button aria-expanded="true">▾ Done</button>' +
        '<div class="selected"><button aria-current="true">Row</button></div>' +
        '<span><span class="status-dot" aria-label="Running"></span></span>' +
        '<button aria-haspopup="menu" aria-expanded="false">Menu</button>',
    );
    expect(a11yReport(d)).toEqual([]);
    d.remove();
  });
});
