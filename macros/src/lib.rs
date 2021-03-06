#![deny(warnings)]
#![feature(plugin_registrar)]
#![feature(rustc_private)]

extern crate rustc;
extern crate syntax;

use rustc::plugin::registry::Registry;
use syntax::ast::{TtToken, TokenTree};
use syntax::codemap::Span;
use syntax::ext::base::{ExtCtxt, MacExpr, MacResult, NormalTT};
use syntax::ext::build::AstBuilder;
use syntax::parse::token::{self, Comma};

/// Executes several closures in parallel
///
/// - This macro is an expression, and will return a tuple containing the values returned by the
/// closures.
/// - One thread per closure (i.e. there is no "load balancing").
/// - This macro will block until all the spawned threads have finished.
///
/// # Expansion
///
/// Consider the following call:
///
/// ``` ignore
/// let (a, b, c) = execute!{
///     || job_0(),
///     || job_1(),
///     || job_2(),
/// };
/// ```
///
/// This is convenience macro that uses `thread::scoped()`, and its expansion looks (roughly) like
/// this:
///
/// ``` ignore
/// let (a, b, c) = {
///     let __thread_0 = || job_0();
///     let __thread_1 = ::std::thread::scoped(|| job_1());  // spawns a thread
///     let __thread_2 = ::std::thread::scoped(|| job_2());  // spawns another thread
///
///     // the current thread takes care of the first closure
///     (__thread_0(), __thread_1.join(), __thread_2.join())
///     // ^~ then blocks until the other two threads finish
/// };
/// ```
///
/// # Panics
///
/// This macro will panic if any of the spawned threads panics.
///
/// # Example
///
/// I'll borrow the binary tree example from Niko's
/// [blog](http://smallcultfollowing.com/babysteps/blog/2013/06/11/data-parallelism-in-rust/).
///
/// ```
/// #![feature(plugin)]
/// #![plugin(parallel_macros)]
/// # extern crate parallel_macros;
///
/// struct Tree {
///     left: Option<Box<Tree>>,
///     right: Option<Box<Tree>>,
///     value: i32,
/// }
///
/// impl Tree {
///     fn sum(&self) -> i32 {
///         fn sum(subtree: &Option<Box<Tree>>) -> i32 {
///             match *subtree {
///                 None => 0,
///                 Some(ref tree) => tree.sum(),
///             }
///         }
///
///         let (left_sum, right_sum) = execute! {
///             // NB Each closure captures a reference and therefore doesn't fulfill `Send`
///             || sum(&self.left),
///             || sum(&self.right),
///         };
///
///         left_sum + self.value + right_sum
///     }
/// }
///
/// fn main() {
///     let tree = Tree {
///         value: 5,
///         left: Some(Box::new(Tree {
///             value: 3,
///             left: Some(Box::new(Tree {
///                 value: 1,
///                 left: None,
///                 right: Some(Box::new(Tree {
///                     value: 4,
///                     left: None,
///                     right: None,
///                 })),
///             })),
///             right: None,
///         })),
///         right: Some(Box::new(Tree {
///             value: 7,
///             left: None,
///             right: None,
///         })),
///     };
///
///     assert_eq!(tree.sum(), 20);
/// }
/// ```
#[macro_export]
macro_rules! execute {
    ($($closure:expr),+,) => ({ /* syntax extension */ });
}

#[plugin_registrar]
#[doc(hidden)]
pub fn plugin_registrar(r: &mut Registry) {
    r.register_syntax_extension(
        token::intern("execute"),
        NormalTT(Box::new(expand_execute), None));
}

fn expand_execute<'cx>(
    cx: &'cx mut ExtCtxt,
    sp: Span,
    tts: &[TokenTree],
) -> Box<MacResult + 'cx> {
    let std_thread_scoped_fn = {
        let segments = vec![
            cx.ident_of("std"),
            cx.ident_of("thread"),
            cx.ident_of("scoped"),
        ];

        cx.expr_path(cx.path_global(sp, segments))
    };

    let mut stmts = vec![];
    let threads = tts.split(|tt| match *tt {
        TtToken(_, Comma) => true,
        _ => false,
    }).filter(|tts| {
        !tts.is_empty()
    }).enumerate().map(|(i, tts)|  {
        let closure = cx.new_parser_from_tts(tts).parse_expr();
        let ident = cx.ident_of(&format!("__thread_{}", i));

        let expr = if i == 0 {
            closure
        } else {
            let fn_name = std_thread_scoped_fn.clone();
            let args = vec![closure];

            // XXX There has to be a simpler way to wrap an expression in `unsafe`
            let block = cx.block_expr(cx.expr_call(sp, fn_name, args));
            cx.expr_block(block)
        };

        stmts.push(cx.stmt_let(sp, false, ident, expr));

        ident
    }).collect::<Vec<_>>();

    let mut is_first = true;
    let expr = cx.expr_tuple(sp, threads.into_iter().map(|thread| {
        let thread = cx.expr_ident(sp, thread);

        if is_first {
            let args = vec![];
            is_first = false;

            cx.expr_call(sp, thread, args)
        } else {
            let args = vec![];
            let join_method = cx.ident_of("join");

            cx.expr_method_call(sp, thread, join_method, args.clone())
        }
    }).collect());

    MacExpr::new(cx.expr_block(cx.block(sp, stmts, Some(expr))))
}
