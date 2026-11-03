import { Component, type ReactNode } from "react";
import { AlertTriangle } from "lucide-react";

// Last line of defence: a rendering bug shows a way out instead of a blank page.
export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="grid min-h-full place-items-center px-4">
        <div className="card w-full max-w-md p-7 text-center">
          <AlertTriangle className="mx-auto mb-3 h-8 w-8 text-rose-400" />
          <h1 className="font-semibold text-slate-100">Something went wrong</h1>
          <p className="mt-2 break-words font-mono text-xs text-slate-500">{error.message}</p>
          <button className="btn-primary mt-5" onClick={() => window.location.reload()}>
            Reload the panel
          </button>
        </div>
      </div>
    );
  }
}
