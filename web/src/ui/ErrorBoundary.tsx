import { Component, type ReactNode } from "react";
import { navigate } from "../lib/route";

type Props = { resetOn?: string; children: ReactNode };
type State = { error: Error | null; resetOn: string | undefined };

export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: null, resetOn: this.props.resetOn };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  static getDerivedStateFromProps(props: Props, state: State): Partial<State> | null {
    return props.resetOn === state.resetOn ? null : { error: null, resetOn: props.resetOn };
  }

  override render() {
    const { error } = this.state;
    if (!error) {
      return this.props.children;
    }
    return (
      <div className="boundary" role="alert">
        <div className="boundary-card">
          <h2>Something went wrong</h2>
          <pre className="boundary-detail">{error.message}</pre>
          <div className="boundary-actions">
            <button type="button" className="btn btn-primary" onClick={() => this.setState({ error: null })}>
              Try again
            </button>
            <button
              type="button"
              className="btn"
              onClick={() => {
                navigate({ page: "bots" });
                this.setState({ error: null });
              }}
            >
              Back to bots
            </button>
          </div>
        </div>
      </div>
    );
  }
}
