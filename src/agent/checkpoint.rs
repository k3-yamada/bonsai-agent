use std::process::Command;use anyhow::Result;
#[derive(Debug,Clone)]pub struct Checkpoint{pub id:usize,pub description:String,pub git_ref:Option<String>,pub timestamp:String}
pub struct CheckpointManager{cps:Vec<Checkpoint>,ctr:usize}
impl CheckpointManager{
pub fn new()->Self{Self{cps:Vec::new(),ctr:0}}
pub fn create(&mut self,desc:&str)->Result<usize>{let id=self.ctr;self.ctr+=1;let now=chrono::Utc::now().to_rfc3339();let git_ref=if is_git(){let o=Command::new("git").args(["stash","push","-m",&format!("bonsai-cp-{id}"),"--include-untracked"]).output()?;if o.status.success()&&!String::from_utf8_lossy(&o.stdout).contains("No local changes"){Some(format!("stash@{{{}}}",self.cps.len()))}else{None}}else{None};self.cps.push(Checkpoint{id,description:desc.into(),git_ref,timestamp:now});Ok(id)}
pub fn rollback(&self,id:usize)->Result<bool>{let cp=self.cps.iter().find(|c|c.id==id).ok_or_else(||anyhow::anyhow!("CP {id} not found"))?;if let Some(r)=&cp.git_ref{let _=Command::new("git").args(["checkout","."]).output();Ok(Command::new("git").args(["stash","apply",r]).output()?.status.success())}else if is_git(){let _=Command::new("git").args(["checkout","."]).output();Ok(true)}else{Ok(false)}}
pub fn rollback_last(&self)->Result<bool>{self.cps.last().map(|c|self.rollback(c.id)).unwrap_or_else(||Err(anyhow::anyhow!("no cp")))}
pub fn list(&self)->&[Checkpoint]{&self.cps}pub fn count(&self)->usize{self.cps.len()}}
impl Default for CheckpointManager{fn default()->Self{Self::new()}}
fn is_git()->bool{Command::new("git").args(["rev-parse","--is-inside-work-tree"]).output().map(|o|o.status.success()).unwrap_or(false)}
#[cfg(test)]mod tests{use super::*;
#[test]fn t_create(){let mut m=CheckpointManager::new();assert_eq!(m.create("t").unwrap(),0);assert_eq!(m.count(),1);}
#[test]fn t_multi(){let mut m=CheckpointManager::new();m.create("a").unwrap();m.create("b").unwrap();assert_eq!(m.count(),2);}
#[test]fn t_git(){assert!(is_git());}
#[test]fn t_rb_err(){assert!(CheckpointManager::new().rollback(99).is_err());}
#[test]fn t_rb_last(){assert!(CheckpointManager::new().rollback_last().is_err());}}
