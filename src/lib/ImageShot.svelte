<script lang="ts">
  // One image from a tool result or a debug device (a screenshot), drawn
  // inline at a bounded height; a click opens it full size over the app,
  // closed by Escape, the Close button or a click outside it. The data is
  // base64 the backend handed over: it only ever becomes a `data:` URL.
  import { lightboxFocus } from './lightbox_focus';

  let {
    mime,
    data,
    alt,
    testid = 'image-shot',
  }: { mime: string; data: string; alt: string; testid?: string } = $props();

  // Only image types become a URL: a result could name any type.
  const src = $derived(/^image\/[a-z0-9.+-]+$/i.test(mime) ? `data:${mime};base64,${data}` : null);
  let open = $state(false);

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      open = false;
    }
  }
</script>

{#if src}
  <button type="button" class="shot" title="Open full size" data-testid={testid} onclick={() => (open = true)}>
    <img {src} {alt} />
  </button>
  {#if open}
    <!-- Review w19-5: focus moves in, stays in, and returns to the shot. -->
    <div
      use:lightboxFocus
      class="lightbox"
      role="dialog"
      aria-modal="true"
      aria-label={alt}
      tabindex="-1"
      data-testid="{testid}-full"
      onclick={(e) => {
        if (e.target === e.currentTarget) open = false;
      }}
      onkeydown={onKey}
    >
      <figure>
        <img {src} {alt} />
        <figcaption>
          <span>{alt}</span>
          <button type="button" class="close" data-testid="{testid}-close" onclick={() => (open = false)}>Close</button>
        </figcaption>
      </figure>
    </div>
  {/if}
{/if}

<style>
  .shot {
    display: block;
    max-width: 100%;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    cursor: zoom-in;
  }
  .shot img {
    display: block;
    max-width: 100%;
    max-height: 420px;
    object-fit: contain;
    border-radius: var(--radius-sm);
  }
  .lightbox {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: grid;
    place-items: center;
    padding: var(--space-3);
    background: var(--scrim);
    /* Focused from script only (tabindex -1), to take Escape: the whole
       screen is not a ring. Close inside it keeps its own. */
    outline: none;
  }
  figure {
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-width: 100%;
    max-height: 100%;
  }
  figure img {
    max-width: 90vw;
    max-height: 82vh;
    object-fit: contain;
    border-radius: var(--radius-sm);
  }
  figcaption {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    font-size: var(--text-xs);
    color: var(--fg);
  }
  .close {
    padding: 2px 10px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--control-bg);
    color: var(--control-fg);
    font: inherit;
    cursor: pointer;
  }
</style>
