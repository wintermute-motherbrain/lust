use crate::builtins;
use crate::parser::{Expr, ExprVal};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

/// An interpreter for Lust code.
pub struct Interpreter {
    /// The global enviroment in which functions are evlauted.
    global_env: Rc<RefCell<LustEnv>>,
}

/// The result of calling a function. If the function is a builtin the
/// result will be a return value, if it is a user defined function
/// then the result will be a new enviroment and expression to
/// evaluate in that enviroment.
pub enum CallResult {
    /// A returned value.
    Ret(LustData),
    /// A new enviroment and data to evalute in it.
    Call(Rc<RefCell<LustEnv>>, LustData),
}

impl Interpreter {
    /// Builds a new interpreter with all of Lust's builtin functions
    /// installed.
    pub fn new() -> Self {
        Self {
            global_env: LustEnv::new(),
        }
    }

    /// Evlalutes an expression from the parser. The expression is
    /// first stripped of location data and then evaluated.
    pub fn eval(&mut self, expr: &Expr) -> Result<(), String> {
        let data = expr.to_data()?;

        Self::eval_in_env(&data, self.global_env.clone())?;
        Ok(())
    }

    /// Evaluates an expression and then prints the result. Used by the
    /// repl.
    pub fn eval_print(&mut self, expr: &Expr) -> Result<(), String> {
        let data = expr.to_data()?;

        println!("=> {}", Self::eval_in_env(&data, self.global_env.clone())?);
        Ok(())
    }

    /// Evaluates an expression in the given enviroment.
    pub fn eval_in_env(expr: &LustData, env: Rc<RefCell<LustEnv>>) -> Result<LustData, String> {
        // The current enviroment we're evaluating in.
        let currentenv = env;
        let currexpr = Self::macroexpand(expr.clone(), currentenv.clone())?;
        Self::eval_without_expansion(currexpr, currentenv)
    }

    /// Evaluates an expression witout performing macro expansion.
    fn eval_without_expansion(
        mut currexpr: LustData,
        mut currentenv: Rc<RefCell<LustEnv>>,
    ) -> Result<LustData, String> {
        loop {
            match currexpr {
                LustData::Symbol(ref s) => break currentenv.borrow().resolve(s),
                LustData::List(ref v) => {
                    // Empty list does not result in function call.
                    if v.len() == 0 {
                        break Ok(currexpr);
                    }
                    let fnres = Self::eval_list(v, currentenv)?;
                    match fnres {
                        CallResult::Ret(v) => break Ok(v),
                        // If this is a call of a user-defined
                        // expression we perform a tail call by
                        // replacing the enviroment and expression
                        // that we're evlauting with the returned
                        // ones.
                        CallResult::Call(env, expr) => {
                            currentenv = env;
                            currexpr = expr;
                        }
                    }
                }
                _ => break Ok(currexpr),
            }
        }
    }

    /// Determines if an expression is a call to a macro.
    fn is_macro_call(ast: &LustData, env: Rc<RefCell<LustEnv>>) -> bool {
        if let LustData::List(ast) = ast {
            if ast.len() == 0 {
                return false;
            }
            let pred = &ast[0];
            match pred {
                LustData::Symbol(ref s) => match env.borrow().resolve(s) {
                    Ok(data) => {
                        if let LustData::Mac(_) = data {
                            true
                        } else {
                            false
                        }
                    }
                    Err(_) => false,
                },
                LustData::Mac(_) => true,
                _ => false,
            }
        } else {
            false
        }
    }

    /// Expands an expression if it is a macro.
    pub fn macroexpand(mut ast: LustData, env: Rc<RefCell<LustEnv>>) -> Result<LustData, String> {
        loop {
            if !Self::is_macro_call(&ast, env.clone()) {
                break Ok(ast.clone());
            }
            ast = Self::eval_without_expansion(ast, env.clone())?;
        }
    }

    /// Evaluates a list.
    fn eval_list(list: &Vec<LustData>, env: Rc<RefCell<LustEnv>>) -> Result<CallResult, String> {
        let pred = Self::eval_in_env(list.first().unwrap(), env.clone())?;
        match pred {
            LustData::Builtin(ref f) => f(&list[1..], env),
            LustData::Fn(ref lf) => Self::eval_funcall(lf, &list[1..], env),
            LustData::Mac(ref lm) => Self::eval_funcall(lm, &list[1..], env),
            _ => Err(format!("invalid list predicate: {}", pred)),
        }
    }

    /// Evaluates a function call. This pretty much just ammounts to
    /// installing its arguments in the enviroment.
    fn eval_funcall(
        func: &LustFn,
        args: &[LustData],
        env: Rc<RefCell<LustEnv>>,
    ) -> Result<CallResult, String> {
        if (func.is_varadic() && args.len() < func.get_min_param_count())
            || (!func.is_varadic() && args.len() != func.params.len())
        {
            if func.is_varadic() {
                Err(format!(
                    "wrong number of arguments for function call. got {} and expected at least {}",
                    args.len(),
                    func.params.len()
                ))
            } else {
                Err(format!(
                    "wrong number of arguments for function call. got {} and expected {}",
                    args.len(),
                    func.get_min_param_count()
                ))
            }
        } else {
            let fnenv = LustEnv::new();
            for (i, param) in func.params.iter().enumerate() {
                if param == "&" {
                    let bind = func.params[i + 1].clone();
                    let val = if i >= args.len() {
                        LustData::List(vec![])
                    } else {
                        let mut res = Vec::with_capacity(args.len() - i);
                        for e in &args[i..] {
                            res.push(Self::eval_in_env(e, env.clone())?);
                        }
                        LustData::List(res)
                    };
                    fnenv.borrow_mut().data.insert(bind, val);
                    break;
                }
                let arg = &args[i];
                fnenv
                    .borrow_mut()
                    .data
                    .insert(param.clone(), Self::eval_in_env(arg, env.clone())?);
            }
            fnenv.borrow_mut().outer = Some(env);
            Ok(CallResult::Call(fnenv, func.body.clone()))
        }
    }
}

impl Expr {
    fn to_data(&self) -> Result<LustData, String> {
        match &self.val {
            ExprVal::Number(f) => Ok(LustData::Number(*f)),
            ExprVal::List(ref v) => {
                let mut res = Vec::with_capacity(v.len());
                for e in v {
                    let data = e.to_data()?;
                    res.push(data);
                }
                Ok(LustData::List(res))
            }
            ExprVal::Id(s) => Ok(LustData::Symbol(s.clone())),
            _ => Err("unsuported form".to_string()),
        }
    }
}

#[derive(Clone)]
pub enum LustData {
    Number(f32),
    List(Vec<LustData>),
    Symbol(String),
    Builtin(fn(&[LustData], Rc<RefCell<LustEnv>>) -> Result<CallResult, String>),
    Fn(Rc<LustFn>),
    Mac(Rc<LustFn>),
}

#[derive(Clone, PartialEq)]
pub struct LustFn {
    pub params: Vec<String>,
    pub body: LustData,
}

pub struct LustEnv {
    pub data: HashMap<String, LustData>,
    outer: Option<Rc<RefCell<LustEnv>>>,
}

impl LustFn {
    pub fn get_min_param_count(&self) -> usize {
        if self.params.iter().any(|i| *i == "&") {
            self.params.len() - 2
        } else {
            self.params.len()
        }
    }

    pub fn is_varadic(&self) -> bool {
        self.params.iter().any(|i| *i == "&")
    }
}

impl LustEnv {
    pub fn new() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::new_with_defaults()))
    }

    fn install_builtin(
        &mut self,
        name: &str,
        func: fn(&[LustData], Rc<RefCell<LustEnv>>) -> Result<CallResult, String>,
    ) {
        self.data.insert(name.to_string(), LustData::Builtin(func));
    }

    fn new_with_defaults() -> Self {
        let mut me = Self {
            data: HashMap::new(),
            outer: None,
        };

        me.install_builtin("quote", builtins::quote);
        me.install_builtin("car", builtins::car);
        me.install_builtin("cdr", builtins::cdr);
        me.install_builtin("cons", builtins::cons);
        me.install_builtin("if", builtins::if_);
        me.install_builtin("eval", builtins::eval);
        me.install_builtin("set", builtins::set);
        me.install_builtin("let", builtins::let_);
        me.install_builtin("fn", builtins::fn_);
        me.install_builtin("macro", builtins::macro_);
        me.install_builtin("macroexpand", builtins::macroexpand);
        me.install_builtin("println", builtins::println_);
        me.install_builtin("negate", builtins::negate);
        me.install_builtin("add", builtins::add);
        me.install_builtin("sub", builtins::sub);
        me.install_builtin("mul", builtins::mul);
        me.install_builtin("div", builtins::div);
        me.install_builtin("lt", builtins::lt);
        me.install_builtin("gt", builtins::gt);
        me.install_builtin("eq", builtins::eq);

        me
    }

    pub fn resolve(&self, id: &str) -> Result<LustData, String> {
        match self.data.get(id) {
            Some(data) => Ok(data.clone()),
            None => match self.outer {
                Some(ref outer) => outer.borrow().resolve(id),
                None => Err(format!("failed to resolve identifier {}", id)),
            },
        }
    }

    pub fn set_global(&mut self, id: String, val: &LustData) -> Option<LustData> {
        match self.outer {
            Some(ref outer) => outer.borrow_mut().set_global(id, val),
            None => self.data.insert(id, val.clone()),
        }
    }
}

impl PartialEq for LustData {
    fn eq(&self, other: &Self) -> bool {
        match (&self, other) {
            (LustData::Number(l), LustData::Number(r)) => l == r,
            (LustData::Symbol(ref l), LustData::Symbol(ref r)) => l == r,
            (LustData::List(ref l), LustData::List(ref r)) => {
                l.len() == r.len() && l.iter().zip(r.iter()).all(|(lhs, rhs)| lhs == rhs)
            }
            (LustData::Fn(l), LustData::Fn(r)) => l == r,
            (LustData::Mac(l), LustData::Mac(r)) => l == r,
            (_, _) => false,
        }
    }
}
// number -> number
// symbol -> symbol
// if -> if cond { then } else { otherwise }
// (set 'name (fn (a))) -> fn name (a, b) -> (return) { body }
impl fmt::Display for LustData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{}", n),
            Self::List(l) => {
                if l.is_empty() {
                    return write!(f, "()");
                }
                write!(f, "(")?;
                for e in &l[..(l.len() - 1)] {
                    write!(f, "{} ", e)?;
                }
                write!(f, "{})", l[l.len() - 1])
            }
            Self::Symbol(s) => write!(f, "{}", s),
            Self::Builtin(_) => write!(f, "<builtin anonymous fn>"),
            Self::Fn(func) => {
                write!(f, "(fn ")?;
                if func.params.is_empty() {
                    write!(f, "()")?;
                } else {
                    write!(f, "(")?;
                    for e in &func.params[..(func.params.len() - 1)] {
                        write!(f, "{} ", e)?;
                    }
                    write!(f, "{})", func.params[func.params.len() - 1])?;
                }
                write!(f, " {}", func.body)?;
                write!(f, ")")
            }
            Self::Mac(func) => {
                write!(f, "(macro ")?;
                if func.params.is_empty() {
                    write!(f, "()")?;
                } else {
                    write!(f, "(")?;
                    for e in &func.params[..(func.params.len() - 1)] {
                        write!(f, "{} ", e)?;
                    }
                    write!(f, "{})", func.params[func.params.len() - 1])?;
                }
                write!(f, " {}", func.body)?;
                write!(f, ")")
            }
        }
    }
}
