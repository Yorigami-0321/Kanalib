import { Component, type ErrorInfo, type ReactNode } from 'react';

interface ErrorBoundaryProps {
  children: ReactNode;
}

interface ErrorBoundaryState {
  error: Error | null;
}

/**
 * 最外层兜底：任一子树抛错时给出可恢复的错误页，而不是整站白屏。
 * 只负责"停在可读状态 + 重新加载"，不吞掉控制台里的原始错误。
 */
export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('[Kanalib] 界面渲染失败', error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="crash" role="alert">
        <p className="crash__eyebrow">Kanalib</p>
        <h1 className="crash__title">界面出错了</h1>
        <p className="crash__hint">这一屏没能画出来。刷新通常可以恢复。</p>
        <pre className="crash__detail">{error.message || String(error)}</pre>
        <button type="button" className="crash__retry" onClick={() => window.location.reload()}>
          刷新页面
        </button>
      </div>
    );
  }
}
