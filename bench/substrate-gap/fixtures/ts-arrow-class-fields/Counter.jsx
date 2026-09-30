import React from 'react';

export class Counter extends React.Component {
  state = { n: 0 };

  handleClick = () => {
    this.bump();
  };

  bump() {
    this.setState({ n: this.state.n + 1 });
  }

  render() {
    return <button onClick={this.handleClick}>{this.state.n}</button>;
  }
}
