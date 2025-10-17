# Building Wasm Components with Python

This cookbook guide shows you how to build WebAssembly components using Python that work with Wassette.

## Quick Start

### Prerequisites
- Python 3.10 or higher
- [uv](https://docs.astral.sh/uv/) - Fast Python package manager

### Install Tools
```bash
# Install uv
curl -LsSf https://astral.sh/uv/install.sh | sh

# Install componentize-py
uv pip install componentize-py
```

## Step-by-Step Guide

### 1. Create Your Project

```bash
mkdir my-python-tool
cd my-python-tool
mkdir wit wit_world
```

### 2. Define Your Interface (WIT)

Create `wit/world.wit`:

```wit
package local:my-tool;

/// Example calculator tool
world calculator {
    /// Add two numbers and return the result
    export add: func(a: f64, b: f64) -> result<f64, string>;
    
    /// Perform a calculation from a string expression
    export calculate: func(expression: string) -> result<string, string>;
}
```

### 3. Generate Python Bindings

```bash
uv run componentize-py -d wit -w calculator bindings .
```

This creates Python bindings in the `wit_world/` directory.

### 4. Implement Your Component

Create `main.py`:

```python
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

import wit_world
from wit_world.types import Err
import json

def handle_error(e: Exception) -> Err[str]:
    """Helper function to convert Python exceptions to WIT errors"""
    message = str(e)
    if message == "":
        return Err(f"{type(e).__name__}")
    else:
        return Err(f"{type(e).__name__}: {message}")

class Calculator(wit_world.Calculator):
    def add(self, a: float, b: float) -> float:
        """Add two numbers together"""
        try:
            result = a + b
            return result
        except Exception as e:
            raise handle_error(e)
    
    def calculate(self, expression: str) -> str:
        """Evaluate a mathematical expression and return JSON result"""
        try:
            # WARNING: eval() is unsafe for untrusted input
            # In production, use ast.literal_eval() or a proper expression parser
            result = eval(expression)
            return json.dumps({"result": result})
        except Exception as e:
            raise handle_error(e)
```

### 5. Create Build Configuration

Create `Justfile`:

```just
install-uv:
    if ! command -v uv &> /dev/null; then curl -LsSf https://astral.sh/uv/install.sh | sh; fi

install: install-uv
    uv pip install componentize-py

bindings:
    uv run componentize-py -d wit -w calculator bindings .

build:
    uv run componentize-py -d wit -w calculator componentize -s main -o calculator.wasm

all: bindings build
```

### 6. Build Your Component

```bash
# Install build tools
just install

# Generate bindings and build Wasm component
just all

# Or run commands manually:
# uv run componentize-py -d wit -w calculator bindings .
# uv run componentize-py -d wit -w calculator componentize -s main -o calculator.wasm
```

### 7. Inject WIT Documentation

To make your component's documentation available to AI agents, inject the WIT documentation into the compiled WASM binary:

```bash
# Install wit-docs-inject (if not already installed)
cargo install --git https://github.com/Mossaka/wit-docs-inject

# Inject documentation into your component
wit-docs-inject --component calculator.wasm \
                --wit-dir wit/ \
                --inplace
```

This embeds the documentation from your WIT files as a `package-docs` custom section in the WASM binary. When Wassette loads your component, it extracts this documentation and uses it to describe your tools to AI agents.

For more information, see the [Documenting WIT Interfaces](./documenting-wit.md) guide.

### 8. Test Your Component

```bash
wassette serve --sse --plugin-dir .
```

## Complete Examples

### Simple Calculator

**wit/world.wit:**
```wit
package local:calculator;

world calculator {
    export add: func(a: f64, b: f64) -> f64;
    export subtract: func(a: f64, b: f64) -> f64;
    export multiply: func(a: f64, b: f64) -> f64;
    export divide: func(a: f64, b: f64) -> result<f64, string>;
}
```

**main.py:**
```python
import wit_world
from wit_world.types import Err, Ok

class Calculator(wit_world.Calculator):
    def add(self, a: float, b: float) -> float:
        return a + b
    
    def subtract(self, a: float, b: float) -> float:
        return a - b
    
    def multiply(self, a: float, b: float) -> float:
        return a * b
    
    def divide(self, a: float, b: float):
        if b == 0:
            return Err("Division by zero")
        return Ok(a / b)
```

### Data Processing Tool

**wit/world.wit:**
```wit
package local:data-processor;

world processor {
    export process-csv: func(data: string) -> result<string, string>;
    export analyze-data: func(data: string) -> result<string, string>;
}
```

**main.py:**
```python
import wit_world
from wit_world.types import Ok, Err
import csv
import json
from io import StringIO

class Processor(wit_world.Processor):
    def process_csv(self, data: str) -> str:
        try:
            reader = csv.DictReader(StringIO(data))
            rows = list(reader)
            return Ok(json.dumps(rows))
        except Exception as e:
            return Err(f"CSV processing error: {str(e)}")
    
    def analyze_data(self, data: str) -> str:
        try:
            items = json.loads(data)
            analysis = {
                "count": len(items),
                "summary": f"Processed {len(items)} items"
            }
            return Ok(json.dumps(analysis))
        except Exception as e:
            return Err(f"Analysis error: {str(e)}")
```

## Error Handling

Python components use WIT's `result` type for error handling:

```python
from wit_world.types import Ok, Err

# Success
return Ok(result_value)

# Error
return Err("Error message")

# Or raise an exception
raise handle_error(exception)
```

## Working with WIT Types

### Type Mappings
```python
# WIT type -> Python type
# s32, s64, u32, u64 -> int
# f32, f64 -> float
# string -> str
# bool -> bool
# list<T> -> List[T]
# option<T> -> Optional[T]
# result<T, E> -> Ok[T] | Err[E]
# record -> dataclass or dict
```

### Complex Types
```python
from dataclasses import dataclass
from typing import Optional, List

@dataclass
class Person:
    name: str
    age: int
    email: Optional[str]

def process_people(people: List[Person]) -> str:
    return json.dumps([p.__dict__ for p in people])
```

## Best Practices

1. **Use type hints** - Python type hints help catch errors early
2. **Handle errors properly** - Always return `Ok` or `Err` for result types
3. **Document your code** - Use docstrings to explain functionality
4. **Test thoroughly** - Validate edge cases and error conditions
5. **Keep it simple** - Avoid complex dependencies that might not work in Wasm
6. **Avoid `eval()` for untrusted input** - Use `ast.literal_eval()` or proper parsers instead of `eval()` to prevent code injection

## Common Patterns

### JSON Processing
```python
import json

def process_json(data: str) -> str:
    try:
        parsed = json.loads(data)
        # Process data
        result = {"processed": True, "data": parsed}
        return Ok(json.dumps(result))
    except json.JSONDecodeError as e:
        return Err(f"Invalid JSON: {str(e)}")
```

### File Processing
```python
def read_file(path: str) -> str:
    try:
        with open(path, 'r') as f:
            content = f.read()
        return Ok(content)
    except FileNotFoundError:
        return Err(f"File not found: {path}")
    except PermissionError:
        return Err(f"Permission denied: {path}")
```

### Data Validation
```python
def validate_input(data: str) -> str:
    if not data:
        return Err("Input cannot be empty")
    
    if len(data) > 1000:
        return Err("Input too large (max 1000 characters)")
    
    return Ok(f"Valid input: {data}")
```

## Troubleshooting

### Build Errors
- Ensure Python 3.10+ is installed
- Verify `componentize-py` is installed via uv
- Check that WIT interface matches your Python class

### Runtime Errors
- Validate all imports are available in Wasm environment
- Check that file paths are correct
- Review Wassette logs for detailed errors

### Import Errors
Some Python libraries may not work in Wasm. Stick to:
- Standard library modules (json, csv, math, etc.)
- Pure Python packages
- Modules explicitly tested with componentize-py

## Full Documentation

For complete details, including advanced topics and more examples, see the [Python Development Guide](../development/python.md).

## Working Examples

See this complete working example in the repository:
- [eval-py](https://github.com/microsoft/wassette/tree/main/examples/eval-py) - Python code execution component

## Next Steps

- Review the [complete Python guide](../development/python.md)
- Check out [working examples](https://github.com/microsoft/wassette/tree/main/examples)
- Learn about [componentize-py](https://github.com/bytecodealliance/componentize-py)
- Read the [FAQ](../faq.md)

## Additional Resources

- [ComponentModel Python Documentation](https://component-model.bytecodealliance.org/language-support/python.html)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [componentize-py Repository](https://github.com/bytecodealliance/componentize-py)
