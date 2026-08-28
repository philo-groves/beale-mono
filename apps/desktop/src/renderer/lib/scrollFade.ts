export interface ScrollFadeMetrics {
  scrollHeight: number;
  clientHeight: number;
  scrollTop: number;
}

export interface ScrollFadeClasses {
  'has-top-fade': boolean;
  'has-bottom-fade': boolean;
}

export function scrollFadeClasses({
  scrollHeight,
  clientHeight,
  scrollTop
}: ScrollFadeMetrics): ScrollFadeClasses {
  const scrollableDistance = scrollHeight - clientHeight;
  const canScroll = scrollableDistance > 8;
  return {
    'has-top-fade': canScroll && scrollTop > 8,
    'has-bottom-fade': canScroll && scrollTop < scrollableDistance - 8
  };
}
