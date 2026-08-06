import React, { useState, useEffect } from "react";
import { createRoot } from "react-dom/client";

/**
 * Error capture for interactive debugging: any runtime error (render,
 * commit, or event-handler) is recorded on window.__appErrors so it can
 * be inspected from the outside (CDP / WebDriver) without a console.
 */
window.__appErrors = [];
window.addEventListener("error", function (event) {
  window.__appErrors.push(String(event.message || event.error));
  if (event.error && event.error.stack) {
    window.__appErrors.push("stack: " + String(event.error.stack));
  }
});
window.addEventListener("unhandledrejection", function (event) {
  window.__appErrors.push("unhandledrejection: " + String(event.reason));
});

function TodoItem({ todo, onToggle, onDelete }) {
  return (
    <li className={"todo-item" + (todo.done ? " done" : "")}>
      <button
        className="todo-toggle"
        type="button"
        aria-pressed={todo.done ? "true" : "false"}
        onClick={() => onToggle(todo.id)}
      >
        {todo.done ? "✓" : "○"}
      </button>
      <span className="todo-text">{todo.text}</span>
      <button
        className="todo-delete"
        type="button"
        onClick={() => onDelete(todo.id)}
      >
        ✕
      </button>
    </li>
  );
}

function FilterBar({ filter, onChange, counts }) {
  const filters = ["all", "active", "completed"];
  return (
    <div className="filter-bar">
      {filters.map(function (name) {
        return (
          <button
            key={name}
            type="button"
            className={"filter-btn" + (filter === name ? " active" : "")}
            onClick={() => onChange(name)}
          >
            {name[0].toUpperCase() + name.slice(1)} ({counts[name]})
          </button>
        );
      })}
    </div>
  );
}

class ErrorBoundary extends React.Component {
  constructor(props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error) {
    return { error: error };
  }

  componentDidCatch(error, info) {
    window.__appErrors.push("React boundary: " + String(error));
    window.__appErrors.push("stack: " + String(error && error.stack));
  }

  render() {
    if (this.state.error) {
      return (
        <div className="error-box">
          <h2>React crashed</h2>
          <pre>{String(this.state.error && this.state.error.message)}</pre>
        </div>
      );
    }
    return this.props.children;
  }
}

let nextId = 4;

function TodoApp() {
  const [todos, setTodos] = useState([
    { id: 1, text: "Build a todo app with React", done: true },
    { id: 2, text: "Bundle it with esbuild", done: true },
    { id: 3, text: "Serve it locally and open it in formal-web", done: false },
  ]);
  const [input, setInput] = useState("");
  const [filter, setFilter] = useState("all");

  const remaining = todos.filter((todo) => !todo.done).length;
  const counts = {
    all: todos.length,
    active: remaining,
    completed: todos.length - remaining,
  };

  useEffect(() => {
    document.title =
      "React Todo (" + remaining + " remaining) — formal-web";
  }, [remaining]);

  function addTodo() {
    const text = input.trim();
    if (!text) {
      return;
    }
    setTodos(todos.concat([{ id: nextId++, text: text, done: false }]));
    setInput("");
  }

  function toggleTodo(id) {
    setTodos(
      todos.map(function (todo) {
        return todo.id === id ? { ...todo, done: !todo.done } : todo;
      })
    );
  }

  function deleteTodo(id) {
    setTodos(todos.filter((todo) => todo.id !== id));
  }

  function visibleTodos() {
    if (filter === "active") {
      return todos.filter((todo) => !todo.done);
    }
    if (filter === "completed") {
      return todos.filter((todo) => todo.done);
    }
    return todos;
  }

  return (
    <div className="react-todo-card">
      <div className="react-todo-header">
        <span className="react-todo-title">React Todo</span>
        <span className="badge working">React {React.version}</span>
      </div>
      <p className="section-desc">
        A todo list rendered by React {React.version} (react-dom createRoot,
        hooks, reconciliation, synthetic events).
      </p>

      <div className="add-row">
        <input
          className="todo-input"
          type="text"
          placeholder="What needs to be done?"
          value={input}
          onChange={(event) => setInput(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              addTodo();
            }
          }}
        />
        <button className="btn btn-primary todo-add-btn" type="button" onClick={addTodo}>
          Add
        </button>
      </div>

      <div className="todo-meta">
        <span className="todo-remaining">
          {remaining} item{remaining === 1 ? "" : "s"} left
        </span>
        <FilterBar filter={filter} onChange={setFilter} counts={counts} />
      </div>

      <ul className="todo-list">
        {visibleTodos().map(function (todo) {
          return (
            <TodoItem
              key={todo.id}
              todo={todo}
              onToggle={toggleTodo}
              onDelete={deleteTodo}
            />
          );
        })}
        {visibleTodos().length === 0 ? (
          <li className="todo-empty">Nothing here — add a task above.</li>
        ) : null}
      </ul>
    </div>
  );
}

function BootStatus() {
  const [phase, setPhase] = useState("mounting");
  useEffect(() => {
    setPhase("mounted");
    window.__reactBooted = true;
  }, []);
  return (
    <div className="boot-status" data-phase={phase}>
      React boot phase: {phase}
    </div>
  );
}

const container = document.getElementById("root");
const root = createRoot(container);
root.render(
  <React.StrictMode>
    <ErrorBoundary>
      <TodoApp />
      <BootStatus />
    </ErrorBoundary>
  </React.StrictMode>
);

window.__reactRoot = root;
