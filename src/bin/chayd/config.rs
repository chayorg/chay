use std::collections::{BTreeMap, HashMap};
use tera;
use toml;

pub fn read_from_file(
    config_path: &std::path::PathBuf,
) -> Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(config_path)?;
    let config = toml::from_str(&content)?;
    Ok(config)
}

pub fn render(
    config: &Config,
) -> Result<BTreeMap<String, RenderedProgramConfig>, Box<dyn std::error::Error>> {
    let mut rendered_config = BTreeMap::new();
    for (program_name, program_config) in &config.programs {
        rendered_config.insert(
            program_name.clone(),
            RenderedProgramConfig::new(&config, program_name, &program_config)?,
        );
    }
    Ok(rendered_config)
}

pub type VarsConfig = HashMap<String, HashMap<String, String>>;

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct LoggerConfig {
    pub command: String,
    pub args: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct ProgramConfig {
    pub command: String,
    pub args: Option<Vec<String>>,
    pub logger: Option<String>,
    pub autostart: Option<bool>,
    pub autorestart: Option<bool>,
    /// Seconds to wait after a program exits unexpectedly before attempted to restart the program.
    pub backoff_delay: Option<u32>,
    pub num_restart_attempts: Option<u32>,
    pub sigkill_delay: Option<u32>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Config {
    pub vars: VarsConfig,
    /// List of programs from the config file, sorted by key in alphabetical order.
    pub programs: BTreeMap<String, ProgramConfig>,
    pub loggers: BTreeMap<String, LoggerConfig>,
}

#[derive(Clone, Debug, Default)]
pub struct RenderedProgramConfig {
    pub program: ProgramConfig,
    pub logger: Option<LoggerConfig>,
}

impl RenderedProgramConfig {
    pub fn command(&self) -> String {
        self.program.command.clone()
    }

    pub fn args(&self) -> Option<Vec<String>> {
        self.program.args.clone()
    }

    pub fn autostart(&self) -> bool {
        self.program.autostart.unwrap_or(true)
    }

    pub fn autorestart(&self) -> bool {
        self.program.autorestart.unwrap_or(true)
    }

    pub fn backoff_delay(&self) -> u32 {
        self.program.backoff_delay.unwrap_or(1u32)
    }

    pub fn num_restart_attempts(&self) -> u32 {
        self.program.num_restart_attempts.unwrap_or(4u32)
    }

    pub fn sigkill_delay(&self) -> u32 {
        self.program.sigkill_delay.unwrap_or(10u32)
    }
}

impl RenderedProgramConfig {
    pub fn new(
        config: &crate::config::Config,
        program_name: &str,
        program_config: &ProgramConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut rendered_config = Self::default();
        let mut vars_renderer = VarsRenderer::new(&config.vars)?;
        rendered_config.program = Self::render_program(program_config, &mut vars_renderer)?;
        if let Some(logger_name) = &program_config.logger {
            if let Some(logger_config) = config.loggers.get(logger_name) {
                rendered_config.logger = Some(Self::render_logger(
                    logger_config,
                    program_name,
                    &mut vars_renderer,
                )?);
            } else {
                return Err(format!("Logger not found: {logger_name}").into());
            }
        }
        Ok(rendered_config)
    }

    fn render_program(
        program_config: &ProgramConfig,
        vars_renderer: &mut VarsRenderer,
    ) -> Result<ProgramConfig, tera::Error> {
        let mut rendered_program_config = program_config.clone();
        rendered_program_config.command = vars_renderer.render_str(&program_config.command)?;
        if let Some(args) = &program_config.args {
            let mut rendered_args: Vec<String> = vec![];
            for arg in args {
                let rendered_arg = vars_renderer.render_str(&arg)?;
                rendered_args.push(rendered_arg);
            }
            rendered_program_config.args = Some(rendered_args);
        }
        Ok(rendered_program_config)
    }

    fn render_logger(
        logger_config: &LoggerConfig,
        program_name: &str,
        vars_renderer: &mut VarsRenderer,
    ) -> Result<LoggerConfig, tera::Error> {
        vars_renderer.add_ctx_vars(&HashMap::from([(
            "program".to_string(),
            tera::to_value(program_name).unwrap(),
        )]));
        let mut rendered_logger_config = logger_config.clone();
        rendered_logger_config.command = vars_renderer.render_str(&logger_config.command)?;
        if let Some(args) = &logger_config.args {
            let mut rendered_args: Vec<String> = vec![];
            for arg in args {
                let rendered_arg = vars_renderer.render_str(&arg)?;
                rendered_args.push(rendered_arg);
            }
            rendered_logger_config.args = Some(rendered_args);
        }
        Ok(rendered_logger_config)
    }
}

#[derive(Clone, Debug, Default)]
struct VarsRenderer {
    tera: tera::Tera,
    tera_ctx: tera::Context,
    chayd_obj: HashMap<String, tera::Value>,
    env_obj: HashMap<String, String>,
}

impl VarsRenderer {
    pub fn new(vars_config: &crate::config::VarsConfig) -> tera::Result<Self> {
        let mut vars_renderer = Self::default();
        vars_renderer.add_system_env_vars();
        let rendered_vars_config = vars_renderer.render_vars_config(vars_config)?;
        for (vars_table_name, vars_hashmap) in rendered_vars_config {
            vars_renderer
                .tera_ctx
                .insert(vars_table_name, &vars_hashmap);
        }
        Ok(vars_renderer)
    }

    pub fn add_ctx_vars(&mut self, ctx_vars: &HashMap<String, tera::Value>) {
        self.chayd_obj
            .insert("ctx".to_string(), tera::to_value(ctx_vars).unwrap());
        // Over-write the previous chayd object in the tera_ctx. Ths is why we have to keep a
        // separate HashMap for chayd_obj. There is no way (that I could find) to modify an
        // existing object in the tera_ctx.
        self.tera_ctx.insert("chayd", &self.chayd_obj);
    }

    pub fn add_env_vars(&mut self, env_vars: &HashMap<String, String>) {
        self.env_obj.extend(env_vars.clone());
        // Over-write the previous env object in the tera_ctx. Ths is why we have to keep a
        // separate HashMap for env_obj. There is no way (that I could find) to modify an
        // existing object in the tera_ctx.
        self.tera_ctx.insert("env", &self.env_obj);
    }

    pub fn add_system_env_vars(&mut self) {
        let mut env_vars = HashMap::new();
        std::env::vars().for_each(|(env_var, value)| {
            env_vars.insert(env_var, value);
        });
        self.add_env_vars(&env_vars);
    }

    pub fn render_vars_config(
        &mut self,
        vars_config: &crate::config::VarsConfig,
    ) -> tera::Result<crate::config::VarsConfig> {
        let mut rendered_vars_config = vars_config.clone();
        for vars_hashmap in rendered_vars_config.values_mut() {
            for value in vars_hashmap.values_mut() {
                *value = self.render_str(&value)?;
            }
        }
        Ok(rendered_vars_config)
    }

    pub fn render_str(&mut self, config_str: &str) -> tera::Result<String> {
        self.tera.render_str(&config_str, &self.tera_ctx)
    }
}
