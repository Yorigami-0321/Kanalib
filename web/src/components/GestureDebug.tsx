import { useEffect, useState } from 'react';
import './gesturedebug.css';

/**
 * `?gestures=1` 时显示最近几次 wheel 的原始值，用来确认触控板到底发的是
 * deltaX 横滑、deltaY、还是 ctrlKey 捏合。默认不渲染、不拦指针。
 */
export function GestureDebug() {
  const [lines, setLines] = useState<string[]>([]);

  useEffect(() => {
    const onWheel = (event: WheelEvent) => {
      const line =
        `dx ${event.deltaX.toFixed(1)}  dy ${event.deltaY.toFixed(1)}  ` +
        `mode ${event.deltaMode}  ctrl ${event.ctrlKey ? 1 : 0}`;
      setLines((current) => [...current.slice(-4), line]);
    };
    window.addEventListener('wheel', onWheel, { passive: true });
    return () => window.removeEventListener('wheel', onWheel);
  }, []);

  return (
    <div className="gesture-debug" aria-hidden="true">
      <strong>wheel debug</strong>
      {lines.length > 0 ? lines.map((line, index) => <span key={index}>{line}</span>) : <span>滚动一下触控板</span>}
    </div>
  );
}
