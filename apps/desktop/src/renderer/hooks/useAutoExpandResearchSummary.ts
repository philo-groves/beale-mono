import { useEffect } from 'react';
import type { RefObject } from 'react';

const SUMMARY_LABEL_SELECTOR = [
  '.session-summary-item > span:not(.session-summary-meta)',
  '.session-memory-type-item > span:first-child'
].join(', ');

export function requiredResearchSummaryWidth(column: HTMLElement): number | null {
  if (!column.querySelector('.session-summary-card')) return null;
  const columnWidth = column.getBoundingClientRect().width;
  if (columnWidth <= 0) return null;

  let largestOverflow = 0;
  for (const label of column.querySelectorAll<HTMLElement>(SUMMARY_LABEL_SELECTOR)) {
    if (label.closest('[aria-hidden="true"]')) continue;
    largestOverflow = Math.max(largestOverflow, label.scrollWidth - label.clientWidth);
  }
  return largestOverflow > 1 ? Math.ceil(columnWidth + largestOverflow + 2) : null;
}

export function useAutoExpandResearchSummary(
  columnRef: RefObject<HTMLDivElement | null>,
  active: boolean,
  ensurePanelWidth: (width: number) => void
): void {
  useEffect(() => {
    const column = columnRef.current;
    if (!active || !column || typeof ResizeObserver === 'undefined') return undefined;

    const measure = (): void => {
      const requiredWidth = requiredResearchSummaryWidth(column);
      if (requiredWidth !== null) ensurePanelWidth(requiredWidth);
    };
    measure();
    const resizeObserver = new ResizeObserver(measure);
    resizeObserver.observe(column);
    const mutationObserver = new MutationObserver(measure);
    mutationObserver.observe(column, { attributes: true, attributeFilter: ['aria-hidden'], childList: true, characterData: true, subtree: true });
    return () => {
      resizeObserver.disconnect();
      mutationObserver.disconnect();
    };
  }, [active, columnRef, ensurePanelWidth]);
}
