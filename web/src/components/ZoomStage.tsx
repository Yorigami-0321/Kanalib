import type { ReactNode, Ref } from 'react';
import { TransformComponent, TransformWrapper, type ReactZoomPanPinchRef } from 'react-zoom-pan-pinch';

export interface ZoomStageProps {
  /** 缩放模式：关闭时滚轮=页面滚动/无限加载；开启时滚轮=缩放、拖拽=平移。 */
  zoomMode: boolean;
  apiRef: Ref<ReactZoomPanPinchRef>;
  onScaleChange?: (scale: number) => void;
  children: ReactNode;
}

/**
 * 整墙缩放容器。非缩放模式下容器保持“普通文档流”（height:auto / overflow:visible），
 * 这样窗口滚动与无限加载不受影响；缩放模式下变成整屏浮动舞台。
 */
export function ZoomStage({ zoomMode, apiRef, onScaleChange, children }: ZoomStageProps) {
  return (
    <TransformWrapper
      ref={apiRef}
      initialScale={1}
      minScale={0.15}
      maxScale={6}
      limitToBounds={false}
      centerOnInit={false}
      centerZoomedOut={false}
      doubleClick={{ disabled: true }}
      wheel={{ disabled: !zoomMode, step: 0.12, smoothStep: 0.004 }}
      panning={{ disabled: !zoomMode, velocityDisabled: true }}
      pinch={{ disabled: !zoomMode }}
      zoomAnimation={{ disabled: true }}
      onTransformed={(_ref, state) => onScaleChange?.(state.scale)}
    >
      <TransformComponent
        wrapperClass="zoom-wrapper"
        contentClass="zoom-content"
        wrapperStyle={
          zoomMode
            ? { width: '100%', height: '100%', overflow: 'hidden' }
            : { width: '100%', height: 'auto', overflow: 'visible' }
        }
        contentStyle={{ width: '100%', height: 'auto' }}
      >
        {children}
      </TransformComponent>
    </TransformWrapper>
  );
}
