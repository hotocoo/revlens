use super::*;

fn names(d: &[Def]) -> Vec<(&str, SymKind, bool)> {
    d.iter()
        .map(|d| (d.qualified.as_str(), d.kind, d.public))
        .collect()
}

#[test]
fn detects_languages() {
    assert_eq!(detect("a/b.ts"), Some(Lang::TypeScript));
    assert_eq!(detect("a/b.d.ts"), Some(Lang::TypeScript));
    assert_eq!(detect("x.mts"), Some(Lang::TypeScript));
    assert_eq!(detect("x.cts"), Some(Lang::TypeScript));
    assert_eq!(detect("x.TSX"), Some(Lang::Tsx));
    assert_eq!(detect("x.py"), Some(Lang::Python));
    assert_eq!(detect("x.pyi"), Some(Lang::Python));
    assert_eq!(detect("x.rb"), None);
    assert_eq!(detect("Makefile"), None);
}

const TS: &str = r#"
import { a } from "./a";
export function add(a: number, b: number): number {
  function inner() { return 1; }
  return a + b;
}
function* gen<T>(x: T) { yield x; }
export default class Svc<T> extends Base implements I {
  private secret(): void {}
  #hidden = 1;
  #priv() {}
  protected prot() {}
  async run(x: string): Promise<void> { await x; }
  testThing() {}
}
class Local { m() {} }
export interface Shape extends Base { area(): number }
export type Id = string | number;
export const mul = (a: number, b: number): number => a * b;
const fn2 = function (x) { return x; };
const notFn = 5;
const test_helper = () => {};
/** doc */
export abstract class Abs { abstract f(): void; g() {} }
"#;

#[test]
fn typescript_defs() {
    let d = defs(Lang::TypeScript, TS).expect("defs");
    use SymKind::*;
    assert_eq!(
        names(&d),
        vec![
            ("add", Function, true),
            ("gen", Function, false),
            ("Svc", Class, true),
            ("Svc.secret", Method, false),
            ("Svc.#priv", Method, false),
            ("Svc.prot", Method, false),
            ("Svc.run", Method, true),
            ("Svc.testThing", Method, true),
            ("Local", Class, false),
            ("Local.m", Method, false),
            ("Shape", Interface, true),
            ("Id", TypeAlias, true),
            ("mul", Function, true),
            ("fn2", Function, false),
            ("test_helper", Function, false),
            ("Abs", Class, true),
            ("Abs.g", Method, true),
        ]
    );
    let add = &d[0];
    assert_eq!(add.signature, "(a: number, b: number): number");
    assert_eq!((add.start_line, add.end_line), (3, 6));
    assert!(!add.is_test && d[7].is_test);
    assert_eq!(d[2].signature, "<T> extends Base implements I");
    assert_eq!(d[10].signature, "extends Base { area(): number }");
    assert_eq!(d[11].signature, "string | number");
    assert!(d.iter().all(|x| x.nodes > 0));
}

#[test]
fn rename_keeps_shape_not_exact() {
    let a = defs(Lang::TypeScript, "function f(a: number) { return a + 1; }").expect("a");
    let b = defs(
        Lang::TypeScript,
        "function g(z: number) { return z + 1; } // c",
    )
    .expect("b");
    assert_eq!(a[0].shape, b[0].shape);
    assert_ne!(a[0].exact, b[0].exact);
}

#[test]
fn tsx_defs() {
    let src = "export const App = (p: { n: string }) => <div>{p.n} é</div>;
export function B() { return <b/>; }
";
    let d = defs(Lang::Tsx, src).expect("tsx");
    assert_eq!(
        d.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
        ["App", "B"]
    );
    assert!(d.iter().all(|d| d.public));
}

const PY: &str = r#"
import os

def top(a: int, b=2) -> str:
    def nested():
        pass
    return "é"

def _private():
    pass

@decorator
@other(1)
def decorated():
    pass

class Svc(Base):
    def __init__(self):
        pass
    def _hidden(self):
        pass
    @staticmethod
    def make(x):
        pass
    class Inner:
        def m(self): pass

class _Priv:
    def pub(self): pass

class TestThing:
    def test_a(self): pass
    def helper(self): pass

async def fetch(url): pass
def test_top(): pass
"#;

#[test]
fn python_defs() {
    let d = defs(Lang::Python, PY).expect("py");
    use SymKind::*;
    assert_eq!(
        names(&d),
        vec![
            ("top", Function, true),
            ("_private", Function, false),
            ("decorated", Function, true),
            ("Svc", Class, true),
            ("Svc.__init__", Method, true),
            ("Svc._hidden", Method, false),
            ("Svc.make", Method, true),
            ("Svc.Inner", Class, true),
            ("Svc.Inner.m", Method, true),
            ("_Priv", Class, false),
            ("_Priv.pub", Method, false),
            ("TestThing", Class, true),
            ("TestThing.test_a", Method, true),
            ("TestThing.helper", Method, true),
            ("fetch", Function, true),
            ("test_top", Function, true),
        ]
    );
    assert_eq!(d[0].signature, "(a: int, b=2) -> str");
    // decorators included in span
    assert_eq!((d[2].start_line, d[2].end_line), (12, 15));
    assert_eq!(d[3].signature, "(Base)");
    let t: Vec<_> = d
        .iter()
        .filter(|d| d.is_test)
        .map(|d| d.qualified.as_str())
        .collect();
    assert_eq!(
        t,
        [
            "TestThing",
            "TestThing.test_a",
            "TestThing.helper",
            "test_top"
        ]
    );
}

#[test]
fn syntax_errors_tolerated() {
    assert!(
        defs(
            Lang::Python,
            "def (:
"
        )
        .is_ok()
    );
    assert!(defs(Lang::TypeScript, "function {").is_ok());
    assert!(defs(Lang::Python, "").expect("empty").is_empty());
}

const RS: &str = r#"
use std::fmt;

pub fn add(a: i32, b: i32) -> i32 { a + b }
fn private<T: Clone>(x: T) -> T where T: Copy { x }
pub struct Point<T> { x: T }
enum Shape { A, B }
pub type Id = u64;
pub trait Area: fmt::Debug { fn area(&self) -> f64; fn unit(&self) {} }
impl<T> Point<T> {
    pub fn new(x: T) -> Self { Self { x } }
    fn hidden(&self) {}
}
impl fmt::Display for Point<i32> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.x) }
}
pub mod inner {
    pub fn exposed() {}
    fn kept() {}
}
#[cfg(test)]
mod tests {
    #[test]
    fn checks() { assert_eq!(1, 1); }
}
"#;

#[test]
fn rust_defs() {
    let d = defs(Lang::Rust, RS).expect("rust");
    use SymKind::*;
    assert_eq!(
        names(&d),
        [
            ("add", Function, true),
            ("private", Function, false),
            ("Point", Class, true),
            ("Shape", Class, false),
            ("Id", TypeAlias, true),
            ("Area", Interface, true),
            ("Area.area", Method, true),
            ("Area.unit", Method, true),
            ("Point.new", Method, true),
            ("Point.hidden", Method, false),
            ("Point.fmt", Method, true),
            ("inner.exposed", Function, true),
            ("inner.kept", Function, false),
            ("tests.checks", Function, false),
        ]
    );
    assert_eq!(d[0].signature, "(a: i32, b: i32) -> i32");
    assert_eq!(d[1].signature, "<T: Clone> (x: T) -> T where T: Copy");
    assert_eq!((d[0].start_line, d[0].end_line), (4, 4));
    assert!(d[13].is_test && !d[0].is_test);
    assert_eq!(detect("src/main.rs"), Some(Lang::Rust));
}

const GO: &str = r#"package shop

import "fmt"

func Add(a, b int) int { return a + b }

func helper[T any](x T) T { return x }

type Cart struct { items []string }

type Store interface { Get(id string) (*Cart, error) }

type (
	ID    = string
	Count int
)

func (c *Cart) Total() int { return len(c.items) }

func (c Cart) size() int { return 0 }

func TestAdd(t *testing.T) { if Add(1, 2) != 3 { t.Fatal("bad") } }
"#;

#[test]
fn go_defs() {
    let d = defs(Lang::Go, GO).expect("go");
    use SymKind::*;
    assert_eq!(
        names(&d),
        [
            ("Add", Function, true),
            ("helper", Function, false),
            ("Cart", Class, true),
            ("Store", Interface, true),
            ("ID", TypeAlias, true),
            ("Count", TypeAlias, true),
            ("Cart.Total", Method, true),
            ("Cart.size", Method, false),
            ("TestAdd", Function, true),
        ]
    );
    assert_eq!(d[0].signature, "(a, b int) int");
    assert_eq!(d[1].signature, "[T any] (x T) T");
    assert_eq!((d[4].start_line, d[4].end_line), (14, 14));
    assert!(d[8].is_test);
    assert_eq!(detect("cmd/x.go"), Some(Lang::Go));
}
